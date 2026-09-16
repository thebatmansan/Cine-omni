# The application

**Status: a real vertical slice.** One command opens a project, serves a
professional shell, and shows the project's own cut and its own media, with
every change travelling through the graph and reaching every other client.

Since the editor slice it is also an editor: a cut can be built from real media,
trimmed, split, lifted and extracted from the interface, and every change shows
what it affected across the production. What it still is not is listed at the
end, in the same words the interface uses.

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

## The editor, and what an edit changes

```
gesture (drag an edge, razor, ⌫, ",")      browser: which frame, which operation
  └─ edit / place                          one JSON-RPC call
       └─ perform_edit                      exact arithmetic, clamped to media,
                                            carried to linked material,
                                            one transaction
       └─ reactor cascade                   script, previs, logistics, timeline,
                                            audio, comp, colour — as the change needs
       └─ reply: outcome + impact           requested vs applied, what changed,
                                            by department, read from the deltas
  └─ `changed` broadcast                   every client refetches once per burst
       └─ impact panel, timeline, record monitor, history — in every window
```

What a user can now do from `cine start`:

| | |
|---|---|
| Build a cut | New sequence at an exact rate; import media by path; mark in/out on the source monitor; insert (`,`) or overwrite (`.`) at the playhead. Picture and sound are placed on the targeted tracks as one linked edit. |
| Trim | Select / Ripple / Roll / Slip / Slide / Razor tools (A T R Y U B). Edge drags show a frame readout and a guide; snapping to runtime-composed cut points and the playhead (S). The status reads *“ripple trim · +12f of +40f (media limit) · +1 linked”* when the media clamped it. |
| Remove | Lift (⌫), extract (⇧⌫), split at playhead (⌘K). |
| See the picture | The record monitor asks the runtime (`picture_at`) which picture of which asset the cut shows, then decodes it. Black says black; media with no declared range says why. |
| Connect to the script | Bind a clip to a scene (inspector or script workspace). Edit a line of dialogue in place. The impact panel names the bound clip and the reactor's reason; the clip is marked on the timeline in every window; *Mark conformed* clears it; undo brings it back. |
| History | One shared undo history, readable in the History tab, undoable from any client. |

### Measured (release binary, two WebSocket clients, `ui/e2e/editor.mjs`)

```
edit reply (client A)                    median 1.7 ms   p95 4.3 ms   max 4.5 ms   n=40
edit → client B notified and refetched   median 5.3 ms   p95 9.7 ms   max 9.7 ms   n=40
  (before TCP_NODELAY:                   median 46.3 ms  p95 55.8 ms  — Nagle + delayed ACK)
single projections                       project 1.1 · timeline 2.7 · history 1.8 · script 1.0 ms
record picture, cold                     2,101 ms  (1,943 ms decode: a walk from the keyframe)
restart                                  sequence, conform flag, rewritten line, binding and
                                         48-step history all recovered from the op log
```

### Defects this slice found and fixed

* **A same-length rewording never reached the cut.** The timeline reactor only
  woke when scene statistics changed. Found by the protocol test written for the
  editor; fixed and regression-tested in `thesis.rs`.
* **A failed reactor cascade left the committed change un-undoable.** The error
  returned before the group reached the undo stack.
* **A serialized time with a zero rate became 24 fps.** Now refused.
* **Editing a clip made from ingested media would have minted a second media
  identity** keyed by path and rewritten what ingest recorded. Material now binds
  the asset it names.
* **Every burst of replies stalled 40 ms** on Nagle's algorithm.

## The production office

The Production workspace shows what the screenplay implies for the shoot, as the
logistics reactor keeps it: a breakdown per scene, a stripboard in the industry
colours with pages in eighths, a call sheet per day, and each scene's shots with
the takes shot against them. Scheduling constraints are edited as a draft and
applied as one reschedule.

Measured against the running application (`ui/e2e/production.mjs`, dev build
with optimised dependencies, two clients):

```
production projection                7.1 ms
dialogue rewrite, full cascade       14.2 ms   impact: Previs, Production, Script, Sound, VFX
scene 1 after the rewrite            2/8 → 13/8 pages, on the stripboard and the call sheet
undo                                 page count restored in every client
```

The Day Out of Days beside it is the cast side of the same board: SW, W, H, WF
and SWF per actor per day, with work and hold counted separately. A hold day —
not shooting, not released, still paid — is what re-ordering the board actually
moves, so it is never folded into a single total. `cine export <dir> dood`
prints the same table.

Strips cannot be dragged, breakdown elements cannot be tagged by hand, there is
no travel or drop/pickup on the DOOD and no budget; the capability says so.

## Colour, and what a view changes

Every project carries an OpenColorIO config — the released ACES 2.0 CG config,
bundled, which is the same file the conformance tests compare against
OpenColorIO 2.5.2. Every media file gets an input colorspace from that config's
*file rules* when it is ingested or relinked, and the graph records which rule
decided (`rule:Movies`) or that a person did (`user`). Nothing is assumed
silently: a file the rules cannot place has no colorspace, and is shown
untransformed.

The Colour workspace chooses the display and view. That choice is session
state, like the open sequence: it is shared by every surface, and the native
window applies it to the picture it is playing. The runtime compiles each
clip's transform — input colorspace → display/view — and bakes it to a 33³
lattice; the window interpolates that lattice in the fragment shader, where the
pixels already are, and prints on the picture which view it is showing and how
far the lattice is from the transform itself.

The browser does not apply it. A browser cannot receive GPU textures, so the
pictures there are decoded frames with no transform, and the panel says so
rather than letting a chosen view imply otherwise.

Measured (`ui/e2e/colour.mjs` against a running `cine start` with real media,
`planar_present.rs` and `view_lattice.rs` on this machine's Intel UHD):

```
colour projection                    7.5 ms
view chosen and broadcast            12.2 ms   every client, unasked
input colorspace with cascade        7.2 ms    undoable; undo restores the rule's decision
GPU vs the same lattice on the CPU   ≤ 1 of 255
the view vs no view                  up to 75 of 255    it is doing something
lattice vs the exact transform       2.1 of 255 in legal code values, 2.8 with sub-black
lattice cost                         88 ms to bake, 0.6 MB on the GPU
```

The native window also has scopes: a waveform and a vectorscope of the picture
*as shown*, through the same conversion and the same lattice the shader uses,
so what the scope reads and what the eye sees differ by no more than the
lattice's own error. They read a sixteenth of the pixels — every fourth each
way — and are measured while the panels redraw rather than per picture, because
playback's budget belongs to playback. The pane says both: how many samples,
and what they cost.

There is no grading — no wheels, curves, qualifiers or windows — no parade,
histogram or HDR scope in the interface, no scopes at all in the browser, and
no analytic transform on the GPU: the lattice is an approximation, and its size
is the measured number above rather than a matter of taste. A 65³ lattice would
halve the error at 558 ms to bake.

## What remains

* **No multi-select, no linked selection, no sync-break indicator.** Edits follow
  links; the interface does not yet draw a broken sync.
* **Three-point editing is source in/out plus the playhead.** No record in/out,
  fit-to-fill or backtiming.
* **Join and replace are protocol-only**; no gesture reaches them.
* **Placing material assumes the sequence can hold it**: a clip at 30 fps in a
  24 fps sequence is placed with its exact duration, which is not a whole number
  of sequence frames. Conform policy (retime vs. frame-snap) is not decided.
* **No playback in the browser**, by design — see *Honesty in the interface*.
  The record monitor steps; it does not play.
* **Component rendering is untested.** There is no DOM test tooling installed
  and no network to add any; the decision logic is tested, the drawing is not.
* **Storyboard, previs, 3D, VFX, audio and delivery workspaces do not exist.**
  Their data is in the graph; their interfaces are not built. Colour has a
  workspace that chooses what to look through and what each file is in, and no
  grading at all.
