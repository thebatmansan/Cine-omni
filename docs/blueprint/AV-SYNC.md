# A/V synchronisation

**Status: FOUNDATION — no audio device exists on this build, so A/V sync is
`UNSUPPORTED` end to end.** What is implemented and verified is the clock
architecture, the audio scheduler, and the proof that a picture paced by an audio
clock lands on the exact sample it should. What is missing is the last piece: a
sink that reaches hardware.

Nothing here is simulated, and nothing claims sound is playing.

---

## The decision: audio is the master clock

Audio cannot be stretched or skipped without being heard. Video can be repeated
or dropped and, up to a point, not noticed. Every serious player therefore paces
picture from sound.

The implementation of that decision is the part worth reading, because it adds
**no second clock at all**. `TimeSource` already meant "where elapsed time comes
from" — the operating system's monotonic clock in production, a hand-driven one
in tests. `AudioTimeSource` is the same interface backed by *the number of audio
frames the device has actually played*:

```text
  sink plays frames ──▶ AudioTimeSource ──▶ PlaybackClock ──▶ Scheduler ──▶ picture
           ▲
           └── AudioScheduler ◀── AudioSource (decoded samples)
```

Three properties follow without being implemented:

* **Pause is free.** A paused device plays no frames, so elapsed time stops and
  the playhead freezes. Nothing has to be told.
* **A stalled device freezes the picture with it**, rather than letting video run
  on ahead of sound.
* **There is no drift to correct**, because there is no second clock to drift
  against. What remains is *offset*, and offset is measured rather than assumed.

The video scheduler was not modified for any of this.

## "Played" means heard, not handed over

A device holds a queue. A sink counting what it was *given* would put the clock
ahead of the sound by the whole device buffer, and every picture would be early
by exactly that much. `AudioSink::played_counter` therefore counts what has left
the device, and `AudioScheduler::position` subtracts what is still queued:

```rust
// written cursor, minus what the device has not yet played
RationalTime::exact(self.cursor - queued, rate)
```

A test writes half a second into a buffer and asserts the playhead is still at
zero, because none of it has been heard.

## The defect this found: nanoseconds are a quantisation

The first version failed a single test — a 23.976 fps picture paced by a 48 kHz
clock — and the reason was in the abstraction, not the arithmetic.

`TimeSource::elapsed` returns `Duration`, a whole number of nanoseconds. **1/48000 s
is not a whole number of nanoseconds**: it is 20833.33… ns. Picture 1 at 23.976
begins at sample 2002, which is 41,708,333.33 ns; flooring gives 41,708,333 ns,
which is *below* the boundary. Every picture therefore changed one poll late, at
every frame, forever.

That is a quantisation, and this project's position on quantised time is already
written down. `TimeSource` gained an exact reading:

```rust
fn elapsed_exact(&self) -> Option<RationalTime> { None }
```

An audio clock returns `Some(frames @ 48000)`; the operating system's clock has
no exact reading to give and returns `None`, keeping its existing path. The
playhead scales the exact elapsed time by the playback rate without dividing —
`ticks` at `num/den` scaled by `pnum/pden` is `ticks * pnum` at `(num*pden)/den`
— so nothing rounds anywhere on the exact path.

## What is refused, rather than approximated

| Situation | Behaviour |
|---|---|
| Source and sink at different sample rates | **Refused.** No resampler exists |
| Source and sink with different channel counts | **Refused.** No channel mapper exists |
| Any playback rate other than 1 | **Muted**, with the reason: no time-stretch, so other speeds would change pitch |
| Reverse | **Muted.** Playing samples backwards is not playback |
| Sink accepts less than offered | `Starved`, counted, and the cursor follows what was *accepted* — never what was offered, which would skip a gap of audio |
| Source exhausted | `EndOfStream` only once the device has drained, because until then the end has not been heard |

Silence that announces itself is honest. Pitch-shifted audio pretending to be
playback is not.

## Tests

`crates/cine-media/tests/av_sync.rs` — **14 tests**, no device, no sleeping, no
timing assertions. The sink is a model of one: the test decides exactly how many
frames it accepts and exactly how many the device has played. The audio itself
encodes its own absolute sample index in every sample, so a test can prove
*which* audio was written, not merely how much.

The central one, `a_video_scheduler_paced_by_the_audio_clock_is_exact`, walks all
240 pictures of a 23.976 fps sequence against a 48 kHz clock — two rates with no
common frame boundary — and for each asserts both that the picture is presented
at its exact sample and that it is **not** presented one sample earlier.

Also covered: the clock is exactly the frames played at 48 kHz and 44.1 kHz;
rounding does not accumulate over four hours at 44.1 kHz; a silent device freezes
the playhead and drops nothing; a seek discards what the device holds; repeated
pumping keeps the queue full with no gaps; a short write is starvation and the
cursor follows it; every refusal above.

## What is missing

* **A device.** `AudioSink` is an interface and this crate ships no
  implementation that reaches hardware. Until one exists **A/V sync on this build
  is `UNSUPPORTED`**, and the shell says so.
* **The route to one, researched but not built.** There are no ALSA, PipeWire or
  PulseAudio development headers on this machine and no `pkg-config`, so `cpal`
  and `alsa-sys` cannot build. The runtime libraries *are* present
  (`libasound.so.2`, `libpipewire-0.3.so.0`, `libpulse.so.0`), there is real
  hardware (`HDA Intel PCH`), and `libloading` is already in the lockfile via
  `wgpu`. Loading `libasound` at runtime therefore needs no headers and no
  linking. That is the plan; it is not done, and the LGPL position on runtime
  loading of a system library needs the same review the FFmpeg strategy got
  before it ships.
* **Measured device latency.** The offset between clock and speaker is modelled
  and tested; it has never been measured against real hardware.
* **Audio from the editorial graph.** `AudioSource` is implemented by tests only.
  Wiring it to `MediaReader` (which already seeks audio sample-accurately) and to
  `cine-audio`'s renderer is the next step.
* **No A/V sync claim.** Not for this build, not for any build, until a device
  exists and the offset is measured on it.
