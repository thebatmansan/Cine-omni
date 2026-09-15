# The application

**Status: a real vertical slice.** One command opens a project, serves a
professional shell, and shows the project's own cut and its own media, with
every change travelling through the graph and reaching every other client.

What it is *not* yet: an editor. Nothing can be cut from the interface. That is
stated here and in the interface itself rather than implied by a toolbar.

---

## The slice, end to end

```
cine start
  └─ opens the project            (op log recovered, lock taken)
  └─ serves the interface         (built UI, same origin as the API)
  └─ serves the protocol          (JSON-RPC over WebSocket, one dispatch)

browser
  └─ project / capabilities / timelines / timeline / media / structure
  └─ draws the composed cut       (positions computed by the runtime)
  └─ GET /frame                   (real decoder → PNG)
  └─ track_state                  → UCG transaction
                                  → op log
                                  → reactor cascade
                                  → `changed` broadcast to every client
                                  → every client refetches
                                  → visible in both windows
```

Each arrow was exercised against the running binary; the measurements are below.

## What the browser is allowed to know

The browser holds a **projection**, never a model:

* It never sums durations. Where a clip sits is composition — the sum of what
  precedes it, at exact rational rates that need not match — and the runtime
  answers it. A second implementation in a language with one number type would
  eventually disagree, and the disagreement would look like a wrong cut.
* Times arrive as `ticks` plus a rate, with `seconds` alongside. The exact pair
  is authoritative; the float exists to turn a time into an x coordinate, which
  is the one place it cannot cause a wrong edit.
* It never writes state locally and pushes it. Every mutation is a call, and
  what redraws the screen is the runtime's `changed` broadcast — the same
  message another client's commit produces. Two windows cannot disagree because
  neither is keeping score.

## Honesty in the interface

The interface renders `capabilities` directly. It does not contain its own
opinion about what works:

| Shown as | Meaning | Example |
|---|---|---|
| verified | proven, with evidence | timeline, live sync, H.264 decode |
| degraded | real but qualified | picture preview — "frame-accurate, not real-time" |
| unavailable | no path exists | real-time playback in a browser, audio |
| refused | deliberately not attempted | codecs that would decode wrongly |

Three consequences that were chosen rather than fallen into:

* **There is no play button in the browser.** Not a disabled one, not one that
  seeks — the transport carries the reason instead. The verified playback path
  decodes into GPU textures and never returns a picture to the CPU, which a
  browser cannot receive. Real-time playback lives in `cine-shell`.
* **Every workspace the application will have is listed**, each showing where it
  stands. Listing only the finished ones would hide the shape of the
  application; letting an unbuilt one open would be worse. A tab that cannot
  open says why, in the runtime's words.
* **Menu entries that have no protocol behind them are disabled with a reason.**
  Ripple, roll, slip and slide exist and are tested in `cine-timeline`; nothing
  reaches them from here yet, and the menu says exactly that.

## Measured

Against the running binary, real HTTP and real WebSocket:

```
startup            project opened at revision 38, 135 nodes, 173 edges,
                   37 operations recovered from the op log
second start       refused, naming the owner's pid and address; the first
                   runtime's revision unchanged and still serving
live propagation   A locks a track → B notified and sees it → B soloes →
                   A notified and sees it, with A's lock still set
undo               A undoes → B notified → B sees the state reverted
first picture      782,721-byte PNG, 720×1280, decoded in 94–114 ms
step to picture 12 486 ms
jump to picture 60 2,595 ms   (a cold walk from the keyframe)
restart            cut and media intact: 1 timeline, 4 tracks, 8 clips,
                   9 media, "Demo Reel v3" 00:00:14:00
```

The picture timings are the honest cost of the browser path: stepping near where
you already are is quick, and jumping into a 189-picture GOP is not. That is why
the capability says *frame-accurate*, not *playback*.

## The rule this phase establishes

> A backend capability is not finished until there is a path to it:
> **protocol → frontend state → visible interface → interaction → test.**

Where a capability is backend-only by necessity, the reason is written down. The
one in this phase is real-time playback: it is native because a frame must not
leave the GPU, and `capabilities` says so to anyone who asks.

## What remains

* **No editing from the interface.** The operations exist and are tested; no
  protocol reaches them. This is the next piece.
* **No selection model beyond one item**, no multi-select, no drag, no snapping
  in the interface — snapping exists in `cine-timeline` and is not yet reachable.
* **The timeline's timebase is not conformed to the media's.** They coincide for
  a cut that references the open media and in general they do not.
* **Component rendering is untested.** There is no DOM test tooling installed
  and no network to add any; the decision logic is tested, the drawing is not.
* **Script, storyboard, previs, 3D, VFX, colour, audio, production and delivery
  workspaces do not exist.** Their data is in the graph; their interfaces are
  not built.
