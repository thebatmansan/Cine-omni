# The editorial workspace

**Status: FOUNDATION.** The docked workspace runs over real playback at 24 fps
with zero dropped frames, measured. What it shows is the *transport and the
picture*; the timeline pane has no timeline wired to it yet, and no editing can
be done from the interface. Those are named at the end rather than implied.

---

## Why native, and not the browser

A decoded frame has to reach the screen without leaving the GPU. `PLAYBACK.md`
has the measurement: reading a converted frame back to the CPU and uploading it
again cost **69.4 ms per picture** against a 41.7 ms budget and dropped 77% of
them. A webview adds another copy on top of that.

So the cutting room is `cine-shell`: winit, wgpu, and egui drawing onto the
shell's own surface with the same device and queue the picture uses. The browser
UI stays as the review surface it was repositioned to be; it is not deleted and
not disparaged, it is simply not where a frame-accurate editor can live.

egui 0.36 with `egui_tiles` for docking. Both were already in the offline
registry, and 0.36 is the series that targets wgpu 30.

## How a frame is assembled

```text
  1. clear the surface
  2. draw the decoded picture, letterboxed into the program pane's rectangle
  3. draw the panels over it, loading rather than clearing
```

The program pane **draws nothing itself**. It reserves its rectangle and reports
it; the caller puts the real picture there through the verified planar path.
Drawing it inside egui would mean a texture round-trip — the exact cost this
architecture exists to avoid.

Letterboxing also fixes a defect recorded in `PLAYBACK.md`: the blit stretched
the image to fill the window, so an anamorphic frame would have looked correct
and been a lie.

## The workspace owns no playback state

`Workspace::ui` is handed a `ViewerStatus` describing what is true and returns a
list of `Command` describing what the editor asked for. It holds no scheduler,
decodes nothing, and cannot move the playhead itself.

Two things follow. The transport is testable without a window. And there is no
second place where playback state lives, which is the usual way an interface and
an engine start disagreeing about what is playing.

`ViewerStatus` is assembled in one place from the scheduler and the decoder, so
no number on screen has a second source.

## What it shows today

* **Transport bar** — play/pause/step/reverse, monospace timecode, picture index
  against the total, transport state coloured by severity, and live dropped and
  stall counters. The counters are on the bar, not hidden in a diagnostics page:
  a viewer that drops frames should say so where the operator is looking.
* **Program** — the decoded picture, aspect-correct, with the colour
  interpretation printed over it and marked when it had to be **assumed**.
* **Timeline** — a zoomable ruler, per-track headers with working **L/M/S/T**
  toggles bound to the editorial session state, clips drawn from the OTIO model,
  a playhead, and click-to-seek. `--timeline <file.otio>` loads a cut into it;
  the toggles change the track's state where `cine-timeline` reads it, so a
  lock set here is the same lock an edit is refused by. A file that will not
  load is reported, never replaced with an empty cut that looks like an empty
  edit.
* **Project** — the open media, its picture count, its keyframes and longest GOP.
* **Inspector** — transport, position, presented, dropped, late, stalls, decode
  mean.
* **Capabilities** — `ShellCapabilities` rendered directly, including the reason
  each unavailable thing is unavailable. This is the one panel that must never
  be omitted.

Keyboard: `space` play/pause, `J`/`K`/`L`, `←`/`→` step, `home` stop.

## Measured

Release build, Intel UHD Graphics (CML GT2) over Vulkan, 1280x720 surface,
`--play 6` on the reference asset:

```
with the workspace:     144 presented, 0 dropped, Playing
                        147 draws, 3.41 ms per draw including the egui pass
without the workspace:  144 presented, 0 dropped, Playing
                        146 draws, 1.35 ms per draw
```

144 pictures in six seconds is 24.0 fps exactly. **The interface costs about
2 ms per frame and does not cost a single picture.**

`--ui-smoke N` drives the real window, surface and egui pass for N frames and
exits non-zero if it cannot — so "the workspace renders" is something a machine
checks rather than something someone remembers seeing. `--play` and `--smoke`
build no interface at all, so the presentation measurements stay measurements of
the presentation path.

### The defect that made this worth measuring

The first working version dropped **38% of its pictures** with the workspace up,
and the counters said why: 456 draws in a six-second run, about 76 a second,
with decode mean rising from 43 ms to 74 ms as egui competed for two cores.

The cause was one line. `window_event` requested a redraw for every event when
the interface was present — and `RedrawRequested` is itself a window event, so
handling a redraw requested another one, unconditionally, forever. The
give-away was in the numbers before the code: there were *more draws than
scheduler polls*, which a loop driven by the scheduler cannot produce.

Excluding `RedrawRequested` took it from 456 draws to 147 and from 55 dropped
pictures to none. A first attempt at the fix — scheduling on egui's declared
repaint delay instead of "does it want a repaint" — was correct in itself and
changed nothing, because it was not the cause. Both are kept: the repaint delay
is the right way to wake for an animation, and it is now also not a spin.

## Two defects the timeline pane found

**An unbounded rectangle hung the application.** `ui.available_rect_before_wrap()`
can be infinite, and the ruler walks ticks until it passes the right edge — with
an infinite edge, forever. It only appeared once a timeline was loaded, because
the empty case returned before reaching the ruler. The pane is now clipped to
something finite *and* the tick loop is capped: a drawing loop should fail to
draw rather than fail to return.

**Rebuilding the panels every frame cost the picture.** With a four-track
timeline drawn, running the interface once per picture measured ~9.7 ms a frame
and pushed decode from 50 ms to 94 ms on two cores — 66 of 144 pictures
presented, 62 dropped, reproducibly, against 144/144 without the pane.

egui's tessellated output can be re-rendered without re-running the interface,
so the panels are now rebuilt on input, on a transport change, and otherwise
about five times a second; in between, the cached interface is drawn over the
new picture. The counters stay live and the picture stops paying for them.

### And a caution about the numbers

After that fix, interleaved runs show **no systematic difference** between
drawing the timeline and not:

```
            no timeline      with timeline
  run 1     140 / 4 dropped  144 / 0
  run 2     144 / 0          144 / 0
```

Consecutive runs of *either* configuration ranged from 60/144 to 144/144
depending on how hot the machine was. The honest reading is that on this
hardware the run-to-run spread is dominated by machine state, and only a
difference that survives interleaving — as the pre-caching one did, and the
post-caching one does not — is a difference in the code.

## What is not implemented, and is not claimed

* **Inside `cine start` the window plays the project's cut**, not a file: the
  sequence is the runtime's flattened edit list (see *One application* below),
  the playhead is sequence time, and every commit by any client reloads it
  without moving the playhead. The standalone `cine-shell <file>` remains the
  reference viewer.
* **Editing happens in the browser surface.** The window changes track state
  through the project; trims, splits and inserts are made in the browser's
  timeline and appear in the window when committed.
* **Dissolves play as cuts.** The edit list counts them and the log says so;
  rendering a mix needs two decodes and a GPU blend, which is not built.
* **No source viewer, bins, search, metadata or scopes.** The docking layout has
  room for them; nothing behind them exists yet.
* **No audio.** No device, no meters, no waveforms. See `AV-SYNC.md`.
* **One asset, one machine.** Everything above is the reference clip on a
  two-core laptop with an integrated GPU.
* **Which sequence the window plays is the most recently created one.** Which
  sequence each surface has open is editor state that is not yet shared.
