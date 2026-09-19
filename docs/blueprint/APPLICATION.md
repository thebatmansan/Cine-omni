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
dialogue rewrite, full cascade       10.6 ms   impact: Previs, Production, Script, Sound, VFX
strip moved, board rescheduled       3.5 ms    the pin is a constraint; undo releases it
scene 1 after the rewrite            2/8 → 13/8 pages, on the stripboard and the call sheet
undo                                 page count restored in every client
```

The Day Out of Days beside it is the cast side of the same board: SW, W, H, WF
and SWF per actor per day, with work and hold counted separately. A hold day —
not shooting, not released, still paid — is what re-ordering the board actually
moves, so it is never folded into a single total. `cine export <dir> dood`
prints the same table.

Each day's call sheet prints: the PDF button on the panel opens
`/callsheet?day=N`, and `cine export <dir> callsheet-pdf --day N --out f.pdf`
writes the same file. It is written by a small PDF writer in this workspace
rather than a dependency, and the tests read it back with poppler and parse it
with Ghostscript instead of trusting that it is a PDF. The boxes a production
expects that this project has nothing behind — weather, hospital, unit base,
transport — are printed as saying so.

A strip can be moved: the buttons on it put the scene on another day, and the
scheduler then arranges everything else around it rather than moving it back,
because the pin is stored with the constraints. Releasing it hands the decision
back. A pin that overfills a day is kept and warned about — the person looking
at the board can see what this cannot.

The breakdown can be corrected. A first assistant adds a prop the script never
mentions, strikes one that is not really there, or overrules a flag the keyword
search guessed; each element shows whether the script said it or a person did.
The corrections live on the scene rather than in the derived sheet, so a
rewrite re-derives the breakdown *around* them — which is the only reason
tagging is worth doing in a system where the breakdown is derived at all.

Strips are moved with buttons rather than dragged, there is no wardrobe,
make-up or stunt category, no travel or drop/pickup on the DOOD and no budget;
the capability says so.

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

A clip can be graded: an ASC CDL — slope, offset, power and saturation — set
in the Colour workspace and applied *in the working space*, which is the
difference between grading a picture and tinting one. The grade is baked into
the same lattice the window is already interpolating, so grading and viewing
are one chain rather than two that can disagree, and the lattice is re-baked
only for the clips whose grade changed.

There are no wheels, curves, qualifiers or windows — no parade,
histogram or HDR scope in the interface, no scopes at all in the browser, and
no analytic transform on the GPU: the lattice is an approximation, and its size
is the measured number above rather than a matter of taste. A 65³ lattice would
halve the error at 558 ms to bake.

### Marks decide the edit

The source viewer and the record monitor both take `I` and `O`, and the keys
apply to whichever is active — the source marks pictures of the media, the
record marks the sequence, and a band in the ruler shows the span. Then any
three of the four points is an edit: mark the source in and out and put the
playhead where it goes, or mark the timeline in and out and mark only where in
the shot to start, and the fourth point is arithmetic.

The arithmetic is the runtime's, not the browser's. `three_point` returns the
plan it resolved — the source range, the record range, whether the edit was
*backtimed* (marked by where it must end, so the record in was derived
backwards) and which points were derived — and the interface reports that
sentence rather than computing a second, drifting copy of it. The edit is
checked against the asset's own available range before anything is committed,
so an edit that would run off the end of the shot is refused with the shortfall
in frames rather than discovered at playback.

Four marks are a different question, and the interface makes the editor answer
it rather than answering quietly: when the two durations disagree the edit is
refused by default, naming the difference, and the `4-pt` control chooses
between trimming the source at the head or the tail, keeping the source's
length by moving a timeline point, and *fit to fill* — which keeps both and
changes the clip's speed.

### Angles

A multicam clip is not a special kind of clip. A clip already carries several
media references with one of them active, and that is exactly what a set of
angles is: `add_angles` attaches assets the project already holds under the
names a production calls its cameras, and switching is choosing a different
one. Every angle is bound to the clip in the graph, so a change to any camera's
media reaches the cut and not only the angle somebody happens to be watching.

Cutting between them is a cut. `1`..`9` at the playhead splits the clip there
and plays the chosen angle from that point on; the material before the switch
keeps playing what it was playing, and the sequence does not change length. The
inspector's chips change one clip without cutting, which is the other thing an
editor wants.

What is refused is the point of the refusal. Angles are aligned by their own
available ranges — two cameras are in sync when their ranges are in the same
time base, which is what timecode gives and what `sync` measures — so an angle
whose camera had not started rolling over this part of the take simply does not
have the frames. Switching to it is refused by name, with how many frames are
missing, rather than cut to and discovered black in a screening.

Angles are lined up by an **offset** each: where that camera's media sits
against the take. It is recorded on the clip, not by rewriting the file's
range, because the same file can be an angle of two takes; switching carries the
take's moment into the other camera's own media through the difference of the
two offsets. *Sync by sound* sets them in the background: every angle's sound is
decoded, cross-correlated with the live angle's, and accepted only if two
independent tests agree — a correlation peak well above the rest, and the two
sounds' loudness rising and falling together at that offset. Measured on real
files, angles of one take agreed 0.9999 and twelve unrelated takes −0.09 to
0.43. The first threshold chosen, on the peak alone, would have lined up five of
those twelve strangers; an angle that does not clearly match is now left alone
and reported.

The record monitor's *Angles* mode shows up to four angles of the clip under the
playhead at once, each a real decode of the picture that angle's own media has at
that moment of the take, or the reason it has none — not rolling yet, stopped
rolling. Clicking one cuts to it there. Like the record monitor it steps; live
four-angle playback needs four decoders in the native player, and is not done.

### Captions

The Captions tab holds a sequence's captions, one language at a time. A new
caption goes where the record marks are, or two seconds from the playhead;
its text is edited in place and committed when the field is left, so every
other window sees it and undo takes it back. Each caption carries the
runtime's own check against the numbers deliveries share — two lines, 42
characters a line, five sixths of a second to seven seconds on screen, two
frames between events, twenty characters a second, no overlap — so the list
says what a QC report would, before there is one.

Times are the sequence's exact frames. IMSC is written with the frame rate
declared and every time an offset in frames, so nothing is rounded; ttconv, an
independent TTML reader run as a test oracle, decodes those documents onto
exactly the frames written, at NTSC rates and an hour in. SRT and WebVTT store
milliseconds, so an import moves each time onto the nearest frame and says how
many moved and by how much, rather than keeping a time the picture cannot show.

Captions are stored in the project beside the sequence, not inside it:
re-importing a cut rewrites the sequence's own contents, and captions are
authored work that has to survive that.

A caption can be linked to the line of dialogue it transcribes, and the link
runs through the project rather than through text: the caption sits over clips,
those clips are bound to scenes, and the lines it can be linked to are the
dialogue of those scenes. *Link to script* links every caption that reads
exactly one of those lines, and leaves anything ambiguous alone; *link to a
line…* offers the rest, most alike first. The script shows **CC** on a line
that has a caption, red when that caption needs review.

Then the script and the cut keep the link honest. Rewrite the line and the
caption is flagged with the words it transcribed and the words the line says
now; cut the line, or its scene, and it is flagged as gone; re-cut the picture
so the caption no longer sits over that scene and it is flagged for that. A
line that only *moved* — an action line inserted above it gives it a new
identity in the script — is followed, not flagged, because nothing it says
changed. Reverting a rewrite clears the flag by itself; *Accept* takes the new
words as the transcription; undo takes back either.

Resolving every link runs on every script or cut change. With 1,200 captions
linked across 120 scenes it costs 49 ms, and a one-line rewrite with every
reactor settles in 183 ms (dev test profile, optimised dependencies). That is
paid on a trim too, and is the number to watch as caption counts grow.

Captions also show as a lane under the picture and sound tracks, from the same
projection the tab reads, so where a caption sits against the cut is visible;
one that needs review against the script, or has delivery issues, is marked,
and clicking it moves the playhead there and opens it in the tab.

SCC, for North American broadcast, is written as CEA-608 pop-on captions and
read back by the same import. What SCC inherits from the line-21 wire is handled
rather than approximated: a caption appears when its End Of Caption code
arrives, so each line starts as many frames early as it has byte pairs ahead of
EOC; a caption's erase goes inside the next caption's load when that load has
already begun; rows hold 32 characters and a longer line is refused by name;
curly quotes, dashes and accents go out as 608's special and extended
characters with their fallbacks, and a character 608 has no form for is
refused; and a sequence at another rate is re-quantised onto 29.97 drop-frame
with the largest move reported. ttconv decodes every caption onto the frame it
was written for — at ten minutes and an hour, where drop-frame counting matters
— and every mapped character. It also found the one real bug: a caption whose
successor started loading before it should clear stayed up until the successor
appeared, because the erase had been left out.

### Rendering the cut: frames or a movie, with its sound

**Jobs → Render** renders the open sequence — the marked frames when both
record marks are set, otherwise the whole cut. The form chooses four things,
each explicit: the output (PNG frames at 8 or 16 bits per channel, or a movie:
H.264 picture and 24-bit PCM sound in a QuickTime file); for a movie, the sound
(the cut's, required, or none); the look (as decoded, or through the project's
display and view); and an output size with how pictures of another shape fit
it (refuse, letterbox, crop, stretch). Captions of one language can be burned
in. The form decides nothing about what is possible: the job is
`render.sequence`, and the runtime's refusal, shown on the job and in the
render list, is the answer.

Below the form, **Renders of this sequence** lists every render on disk with a
state — `current`, `stale`, `invalid` or `failed` — and the reasons. Each can be
previewed (a picture decoded from the delivered movie, not from the cut),
downloaded, and its manifest opened.

**How a frame is made.** Every record frame is resolved by the same edit list
and the same picture arithmetic the record monitor uses
(`playlist::picture_index`), so a retimed clip or a switched angle renders as
it plays. Then: decode → the project view, by the bake the native window uses
(`cine_color::view`), with its measured lattice error recorded → fit to the
output size by a Lanczos-3 resampler that averages rather than skips when it
shrinks (`cine_comp::ops::resample`), with what was kept and where it went
recorded per source → captions on the IMSC grid → written.

**How the sound is made.** The mixer the native window plays
(`cine_media::sequence_audio`), at 48 kHz stereo: retimed clips as varispeed
or with pitch kept, as their retime says; files at other rates converted by the
same kernel; muted tracks excluded. Sound is divided among pictures by absolute
sequence position — picture `k` carries the samples from
`floor(k·den·48000/num)` to `floor((k+1)·den·48000/num)` — so a movie never
drifts from its picture by a sample and a marked range carries exactly the
samples a whole-cut movie carries for those frames. Clipped samples are counted
in the manifest and shown, never limited silently.

**Where precision is lost, traced:**

| Stage | Precision | Evidence |
|---|---|---|
| Decode | 8-bit 4:2:0 — all this build decodes | `capability.rs` |
| YUV → RGB | 32-bit float, unclamped (limited-range excursions kept) | `frames::rgba_f32` |
| View | float through a 33³ trilinear lattice; worst error measured per bake and recorded | `cine_color::view` |
| Scale, captions | float, unclamped | `resample.rs`, `captions.rs` |
| PNG 16 | clamped to [0, 1], 1/65535 | `render_movie.rs`: every sample equals the view's output at 16 bits; none lies on the 8-bit grid |
| PNG 8 | clamped, 1/255 | |
| Movie | clamped, 8-bit BT.709 limited range, chroma averaged over 2×2, lossy H.264 | `rgb_to_yuv8_cpu` inverts the reference conversion within one code value |

The first stage is the limit for camera originals: nothing downstream can add
precision the decoder did not produce. A 10-bit or float source path is a
decoder capability this build does not have.

**What it refuses, by name:** an offline clip or a picture the decoder does not
produce (never black in its place); pictures of different sizes with no output
size, or a different shape with no fit chosen (never a silent stretch or crop);
a view some clip cannot be baked into; sound asked for where the cut has none
(never a silent track presented as its sound), a sound clip that cannot be
played, or sound in PNG frames; a movie through a display whose primaries it
cannot declare — P3, Rec.2100 — rather than a Rec.709 label on it; an odd movie
size; captions in a language the sequence does not have.

**Colour the movie declares.** Samples are BT.709 matrix, limited range. As
decoded, the primaries and transfer are those the sources declare when they all
agree, otherwise "unspecified" (H.273 code 2) — not a guess. Through a view,
those of the display: sRGB (transfer 13) or Rec.1886/709 (transfer 1).

**Whether a render is still current** (`provenance.rs`). The manifest records
one fingerprint per thing the render read: the part of each clip inside the
rendered range and the media at its start, each file's content hash and path,
the captions in range, and the colour (config, the display and view pinned at
render time, inputs and grades). Asking later recomputes the same fingerprints
and re-hashes each file on disk. So a trim where the range does not reach, a
new sequence or a script edit leaves a render `current`; a trim that changes a
rendered frame makes it `stale` with "the cut of … changed", undoing it makes it
`current` again, a changed file on disk is named, and a whole-cut render also
goes stale when the cut's length changes. A render whose files are missing is
`invalid`; a job that produced nothing is `failed` with its refusal.

**Checked, and where:**

* `render_movie.rs` — a movie of A, a gap and B read back picture by picture
  (bar positions) and sample by sample: A's and B's sound bit-exact, the gap
  silent; a marked range has exactly its sound; `sound: false` writes no sound
  track; a 2× varispeed clip shows every second picture and its ticks land
  **0 samples** from their pictures; scaling, letterbox bars and the recorded
  placement; 16-bit precision; cancel mid-render leaves nothing; staleness as
  described; the served movie is byte-identical to the file; path traversal
  refused.
* `mov_roundtrip.rs` — the movie writer against this build's decoders and
  against GStreamer's `qtdemux`, an independent implementation, which reports
  the same tracks, rate and duration and returns the exact sound bytes.
* `ui/e2e/render.mjs` — the real application: `cine new`, `cine bars`,
  `cine start`, and headless Chrome over the DevTools protocol choosing a movie
  in the Jobs tab, rendering, previewing, downloading, being refused a
  different shape, watching the render go stale after an edit and current after
  undo, with no uncaught page errors.

**Measured** (dev profile, 320×180, this laptop): 46 frames with sound in
1,171 ms; submit to a current movie visible in the browser, 48 frames with
sound, 1,074 ms and 3,302 ms in two runs of `render.mjs` — the spread is this
laptop under load, and neither is a claim about rendering speed. Nothing here says anything about 1080p or 4K rendering speed,
which has not been measured.

**Defects found on the way.** Reading 24-bit sound — the ordinary format of
production audio — produced frames with no channels, heard as silence; 8-bit,
unsigned and 64-bit float layouts likewise (`sound_formats.rs`). The container's
`colr` atom was never read, so files declaring colour only there — including
every movie this build writes — decoded as "BT.601, assumed" at SD heights. A
colour description sent in an HTTP header arrived in the browser as `Â·`. A
whole-cut render re-derived its range from the current cut, so its staleness
reason named the wrong frames. Each has a regression test.

Not yet: other codecs (ProRes, DNxHR, AAC), sound other than 48 kHz stereo
24-bit, more than 8-bit sources, timecode tracks, and rendering on the GPU.

### Proxies, and choosing what plays

**Play** in the menu bar is one choice for the whole application — Originals,
Proxies or Auto — shared like the open sequence, so the browser and the native
window read the same files. **Check media** looks for every original on disk
now and records what is there; offline is a normal state, not an error.

A proxy is made from the media inspector: choose a height, press **Make proxy**,
and the job appears in Jobs. It decodes the original, scales it with the
Lanczos-3 resampler, and re-encodes it as a QuickTime movie with the original's
frame count and frame rate — picture `i` of the proxy is picture `i` of the
original, so nothing has to be conformed later — and with the original's sound,
sample for sample, when it has one or two channels.

**A proxy is never the original.** It is a `media.derivative` node with a
`DerivedFrom` edge to the original's asset, so no clip can bind to it and no
render can read it. It records what it was made from (the original's content
hash), how (its recipe, including a recipe version), what it is (its own hash,
size, probed streams), whether it carries sound, and which job made it.

**What plays** is one function (`proxy::choose`) that every surface calls:

* **Originals** — the original.
* **Proxies** — the largest current proxy; otherwise the original, with the
  reason shown.
* **Auto** — a current proxy when the original is offline, cannot be decoded by
  this build, or is larger than HD; otherwise the original.

A proxy that is `stale` (made from another original, by an older recipe, or
changed on disk) or `missing` is never read, by any surface, and `/frame`
refuses to serve it. The record monitor shows a **PROXY** badge and the reason
whenever it is not showing the original. Renders always read originals, whatever
is being watched (`proxy_protocol.rs`).

Sound follows picture only when the proxy has sound. A proxy of a six-channel
original carries none — there is no downmix — so the clip's sound stays on the
original and the edit list says so.

**Checked:** `proxy_protocol.rs` — a proxy's pictures match its original's and
its sound is bit-exact; asking again does not remake it; a stale proxy is never
played or served; Auto prefers the original until it is offline or over HD; a
six-channel original's sound stays on the original; refusals (not smaller,
offline) and cancellation leave no files and no record; a failed job retried
succeeds; renders read originals. `ui/e2e/proxy.mjs` — the real application:
select the media, make a 360p proxy, watch it become current, switch to
Proxies, see the monitor say PROXY, break the file and watch the application
stop trusting it.

**Measured** (dev build, CPU, this laptop): 640×360 → 320×180 with sound, 4.4
pictures/s; 1920×1082 → 958×540, 1.9 pictures/s; in the browser, submit to a
current 640×360 proxy of 24 HD pictures, 4.9–6.7 s. Per stage for the HD proxy:
scale 195 ms, YUV→RGB 90 ms, encode 96 ms, decode 69 ms per picture — so
scaling dominates, and it is 1.7× faster than it was after specialising the
resampler's inner loop, with the byte-path colour conversion 2× faster than
building float planes first. Both are still CPU, single-threaded, in a dev
build; nothing here is a claim about release performance or about real-time.

### What this build can do with media, audited

Two tables in `cine_media::capability` are the only place a capability is
stated: the codec registry (may we decode this stream) and `MEDIA_PATHS` (does
this *path* exist, and how far). The `capabilities` method reads both, so the
Capabilities panel, the error messages and this document cannot drift from the
code — and nothing can claim what neither table states.

Media paths carry one of: `verified` (proven against real media by a named
test), `implemented` (built and used, not proven end to end), `partial` (works
within stated limits), `prototype`, `refused` (deliberate, with the reason),
`missing` (not built), `blocked` (waiting on something named). A `verified` row
that does not name its evidence fails a test.

The audit of 2026-09-18 recorded, among 27 paths: MP4/MOV read, QuickTime
write, WAVE read, H.264 8-bit decode, exact duration and media time,
frame-accurate seek, colour from stream and container, PCM at every width,
proxy generation and playback, render input, content identity and presence —
all verified. Partial: Matroska (sound only — an MKV's picture track is
neither described nor decoded), variable frame rate (detected and refused, not
conformed), alpha (stills only), bounded decoding (stills and EXR are read
whole). Missing: MXF, image sequences as media, high-bit-depth decode,
timecode, HDR mastering metadata, camera metadata, waveforms. Blocked: float
picture as media and HDR viewing, both on the decoder.

Two entries were corrected by the audit itself, because the tests written since
now prove them: Baseline H.264 (this build's own encoder round-trips through
its decoder) and 24-bit and float PCM.

### Media that moved, and media that only looks the same

An asset's identity is its content hash; its path is where the file was last
seen. **Check media** (menu bar) fingerprints the file at each recorded path
and reports what is actually there. **Relink** — in the media inspector — takes
a folder, and the runtime looks for *that content* under it.

What a scan answers, per asset: `relinked` (found by content; the asset keeps
its identity, so every clip, proxy, grade and render still refers to it),
`already here`, `replaced` (a file is at the path but is not this media —
reported, never accepted, because replacing media is a separate decision), or
`not found` (with the files that share the name listed as near misses, and
their hashes, so the refusal is explicable). Two paths holding identical
content are not an ambiguity: one is used, the rest reported as duplicates.

When the project holds a verified SHA-256 (from `media.checksum`), the winning
candidate is hashed in full and must match it; the relink is then recorded as
`exact` rather than `fingerprint`. A person can also relink to one named file,
which is refused with both hashes when that file is not this media.

**Defect found by this work.** "Check media" asked only whether *a* file was at
the path, so a different take written over an original reported the media as
online — and the application would have played the wrong pictures without a
word. It now compares content, and a namesake is `replaced`, not online
(`relink_protocol.rs::checking_media_does_not_call_a_namesake_online`).

**Also fixed:** render provenance fingerprinted media by hash *and path*, and
the sequence fingerprint included each clip's path and online flag, so relinking
media made every render of it stale although nothing about the render's content
had changed. Fingerprints are now by content and by asset.

**Checked:** `relink_protocol.rs` — moved and renamed media is found by content;
a different take of the same name is refused, by scan and by hand, with both
hashes; media that is nowhere stays unresolved and says what was looked for; two
copies are not an ambiguity; a verified checksum makes the relink exact; and
after a relink the clip still plays, the proxy is still current, and the render
is still current. `ui/e2e/relink.mjs` — the real application: the file is moved
and a namesake left in its place, **Check media** shows the replacement, a
folder is typed, and the media comes back under its new name (229 ms from typing
to relinked, dev build).

### Image sequences, and the high-bit-depth input they bring

**Project → Import media → "Read as an image sequence"** ingests a numbered run
of EXR, PNG, TIFF or JPEG files as *one asset*. Left unticked, a numbered file
with siblings is still read as the run, and one on its own is one still — and
the answer says which was done, rather than leaving it to be inferred.

A run of files has no rate of its own, so a sequence takes the rate chosen at
import, and its length is the numbers it covers. Its identity is the run — the
pattern, the range, its ends — so any member identifies the same asset, and a
frame added at the end makes it a different run. A number inside the range with
no file is a **hole**: it is part of the length, it is reported at import, and
asking for it answers with the file that is missing, never with black.

**This is the build's high-bit-depth input.** EXR is read as 32-bit float
whatever it was stored as, unclamped, so scene values above 1 and below 0 reach
the pipeline intact; 16-bit PNG and TIFF arrive as 16-bit. Nothing converts
colour on the way: an EXR says nothing in H.273 terms, so its frame carries no
colour code points and the project's colour rules decide what it is in, as for
any other media.

Everything that reads pictures — the monitors, proxies, renders — now asks the
same three questions of a movie or a run (`cine_media::Pictures`): how many, at
what rate, give me number *i*. What differs travels with the answer instead of
being hidden: a frame arrives in the format its source holds, and the monitor
shows a float picture clamped **and says so**.

**What refuses, and why.** A proxy is 8-bit video, and a render writes
display-encoded output; turning scene-linear float pictures into either needs a
view transform this path does not apply. Both refuse by name rather than
clipping every highlight to white. That is the honest state: EXR can be
imported, cut, identified and previewed, and cannot yet be delivered.

**Checked:** `image_sequence.rs` — a run is one asset with its own rate and
length; EXR pixels reach the frame as the floats they are (a value at 1e-5, a
highlight at 6.25 and above, a negative sample, all surviving); a 16-bit PNG is
not quantised to 8; a hole refuses by name and the run continues past it.
`sequence_protocol.rs` — ingest, identity from any member, cutting, the record
monitor, the clamped-and-said-so preview, single files, and both refusals.

### Scene-linear pictures, and what a view is for

Scene light is unbounded. A highlight at six times display white is ordinary in
an EXR, and the whole job of a view transform is to decide what such a value
looks like on a display. So the colour path now has two methods, and the input's
own encoding chooses between them (`cine_color::view::view_for`):

* **display-referred input** (video, log) — the 33³ lattice, as before: what the
  window's shader interpolates, with its error measured and recorded.
* **scene-linear input** — the transform applied **exactly**, per pixel, by the
  processor itself. A lattice over code values would have to clamp unbounded
  light or stretch its domain absurdly; neither is a picture anyone should
  deliver. The exact path has no lattice error, and the manifest records `0`
  rather than a number that would be a fiction.

`SOURCE → decode → source precision → scene-linear → view → display` is
therefore a real chain, and the source representation is never overwritten by a
display one: no view is baked into an asset, and a render through a view reads
the original's own values every time.

**What this makes possible.** Scene-linear pictures now deliver: through the
project's view to any display-encoded output, or as EXR unchanged. **Jobs →
Render → EXR** writes 32-bit float RGBA frames, unclamped and untransformed —
a round trip returns the source's scene values, highlight and shadow alike.

**What still refuses, and why it is the right answer.**

* *Scene-linear as decoded into display output* — it has no display values;
  writing it would clamp every highlight to white. Render it through a view, or
  as EXR.
* *A proxy of scene-linear media* — a proxy stands in for its original wherever
  that original plays, so a baked view inside it would be applied a second time
  by the window. There is no correct 8-bit stand-in for scene light.
* *Captions burned into EXR* — burn-in draws display white, which is not a scene
  value. Deliver the captions as a file, or render a display-encoded output.
* *A movie through a P3 or Rec.2100 display* — the writer can only declare
  Rec.709 primaries, and labelling a P3 picture Rec.709 is the substitution this
  refuses.

**Looking at it.** The source monitor has a **View** button: off, the file's own
values (a float picture is shown clamped, and says so); on, the project's
display and view, through the same function a render uses — so a preview and a
delivery cannot disagree about colour.

**HDR, stated exactly.** Rec.2100 PQ and HLG views work and differ from the SDR
view as they must — measured, scene 4.0 → **0.6398** through ACES 2.0 HDR 1000
nits (P3 D65), **0.8999** through sRGB. What no surface here can do is *present*
HDR: the window and the browser are SDR. So a PQ preview is labelled "these are
Rec.2100-PQ code values, not an HDR presentation", and no path claims otherwise.
No HDR media can be decoded yet either, and no mastering metadata is read or
written.

**Checked:** `cine-color/tests/scene_linear_view.rs` — scene-linear takes the
exact path and video takes the lattice; highlights roll off in order without
clipping flat; deep shadows keep their order; negatives stay finite and do not
sit above black; a whole picture equals pixel-by-pixel and alpha is untouched;
an HDR view is a different transform from the SDR one.
`sequence_protocol.rs` — as-decoded delivery refuses with its reason; through
the view a ramp from deep shadow to a highlight several stops up delivers
monotonically, with the highlight *not* clamped flat; EXR delivery returns the
scene values; captions on EXR and proxies of scene-linear refuse.

**Measured** (dev build): four 32×18 scene-linear frames through the exact view
to 16-bit PNG, 43 ms. The exact path costs per pixel what a lattice lookup does
not; nothing here is a claim about 4K throughput, which has not been measured.

### Waveforms

A timeline draws sound as an envelope, and the envelope is the recording's own:
**media.waveform** decodes the file once and keeps its peaks. Four zoom levels,
256 to 131,072 samples a bucket, powers of eight apart, so any zoom finds a
level within a factor of eight of what it wants and the pyramid costs 14% more
than its base. Each bucket keeps a **minimum and a maximum** per channel, not
one magnitude: folding the halves together would hide asymmetry, DC offset and
one-sided clipping, which are what someone looks at a waveform to see.

The source is decoded frame by frame and folded in as it arrives, so an hour of
audio is never resident; only the peaks are. Measured: four seconds of 48 kHz
24-bit stereo, **43–78× real time**, 6.9 kB of peaks — an hour would be about
6 MB against 691 MB of audio.

A clip draws its **own media range**, asked for in the source's samples, at as
many columns as the clip is wide — not the start of the file. When there is no
overview, or it is stale, nothing is drawn: an invented envelope is a picture of
sound that does not exist. The media inspector shows the state and makes one.

It belongs to its original: a `media.derivative` with a `DerivedFrom` edge,
never media of its own. It goes stale when the original's content changes, when
the recipe version changes, or when its own file changes — and **survives a
relink**, because moving media does not change it.

**One definition of derived state.** Proxies, waveforms and anything derived
later share `crate::derived::status`: missing when its file is gone, stale when
it was made from other content, by an older recipe, or its own file changed.
That is also why a relink leaves derived data alone: identity is content, and
the content did not change.

**Checked:** `waveform_protocol.rs` — a written recording of silence, a loud
second and a quiet one comes back with those levels in its peaks, per channel,
with the left twice the right as written; a wide window uses coarser buckets
and a narrow one the finest; windows start on a bucket edge; the job is not run
twice for work already done; the overview survives a relink and goes stale when
its file is damaged, refusing to be drawn from; media with no sound is refused;
a cancelled overview leaves no files and no record. `cine-media` unit tests hold
the levels, the window arithmetic and the file format.

### Importing media, and what follows an import

**Project → Import media** is a job now (`media.ingest`), and it does the whole
of becoming an asset:

```text
discover → identify → probe → evaluate → register → dispatch derived work
```

*Discover* one file or the numbered run it belongs to; *identify* by content,
through the same function relinking and proxies ask; *probe* container, codecs,
sizes, rates, durations, colour and sound; *evaluate* what **this build** can do
with what was found, from the capability registry rather than from hope;
*register* in one transaction, so the asset exists whole or not at all; then
*dispatch* the derived work asked for as jobs of its own.

The dialog asks what should follow: **thumbnail** and **waveform** default on,
because they are seconds of work and are what a browser needs to show media at
all; **proxy** is off unless asked for, because it is minutes of work and
gigabytes of disk, and no policy here decides that for someone. What cannot be
done is said rather than skipped: a WAV gets no thumbnail and the answer says
why.

Cancelling is honoured up to the moment of registration — before it, nothing has
been written; after it, the asset is real and the import finishes. There is no
half-registered asset to find later. A file that is not media, or is not there,
fails with the file named and registers nothing.

The import job's key is the path **and** what is at it (size and modification
time), so importing the same file twice is one piece of work, while a different
file written over that path is not answered with the first import's asset.

#### Measured in the real application (`ui/e2e/ingest.mjs`, dev build)

`cine new`, `cine bars`, `cine start`, then headless Chrome driving the actual
interface: open **Project ▸ Import media…**, type a path, tick thumbnail,
waveform and proxy, press Import.

| step | measured |
|---|---|
| Import pressed → the asset visible in the media browser | 408 ms |
| the waveform mark → the proxy mark, for 24 pictures of 1280×720 | 5 520 ms |
| the four jobs the runtime reported | Import 0.04 s (io), Thumbnail 0.33 s, Waveform 0.16 s, Proxy 540p 5.85 s |

What the test holds, and would fail on: the browser holds **no** row for the
file before the import; exactly **one** row after it, not one per derived thing;
**no** proxy mark before the proxy job has succeeded; the waveform mark before
the proxy mark, in that order; and the import and each dispatched job present in
Jobs as the runtime's own records. Nothing in the interface is allowed to
anticipate the runtime — a timer or an optimistic mark would fail the third and
fourth of those.

**A defect this found.** The dialog closed the moment the import was submitted,
so the line naming what had been submitted and where to watch it was never
shown — dead code, and no confirmation for the person. Cards arrive one after
another, so the dialog now stays, says what was submitted, clears the path for
the next one, and offers **Close** rather than Cancel once something is in.

### What derived data depends on, and what it survives

`derived_provenance.rs` walks the whole matrix in one place, because the value
of recording provenance is an answer that is neither optimistic nor paranoid:

| what happened | what follows |
|---|---|
| media moved or renamed, relinked | proxy, waveform and render all stay **current** |
| the original is deleted | derived data stays current; the media is **offline**; Auto plays the proxy and says why; reading the original refuses |
| a namesake with other content sits at its path | the media is **not online** (`replaced`); nothing derived changes |
| that other content is imported | it is a **different asset**, with nothing derived |
| the derived file itself changed | that one thing is **stale**, and nothing else |
| the recipe version changed | that one thing is **stale**, and playback will not read it |
| an edit outside the rendered frames | the render stays **current** |
| an edit inside them | the render is **stale**; undo makes it current again |
| the source's bytes changed where the project still vouches for it | the render is **stale**, naming the file |

**The rule the last two rows turn on.** A fact about a file on disk is a reason
a render is out of date only where it *contradicts what the project claims*.
Where the project has already recorded that the media is away — offline, or a
namesake its relinking refused — the same fact says only that the render cannot
be **made again**; the render is still exactly what it was made from. Where the
project still says "this media is its content, at this path" and the bytes there
are not that content, something changed underneath and remaking now would give a
different picture: that is staleness. The answer carries `reasons` (out of
date), `notes` (true without meaning that) and `remakeable`, and the render list
shows all three, notes in a quieter voice than reasons.

**Defects this audit found and fixed.**

* A render was called *stale* when its source went offline, and again when a
  namesake sat at its path. Both are now notes.
* Over-correcting that made a render whose source bytes really had changed read
  as *current*. The rule above is what separates them; both cases now hold in
  the same run.
* `/frame` served a picture decoded from whatever file sat at an asset's path,
  even where the project had already recorded that file as **`replaced`** — a
  stranger's take, served as this one. Every read of an original now asks
  `relink::unreadable` first, which answers from the project's record rather
  than from a fresh hash on every frame.
* The import job's key was the path alone, so a different file written over an
  imported path was answered with the first import's asset.
* `media.thumbnail` wrote straight to its cache path and then trusted the path's
  existence, so a write interrupted halfway was taken for a finished thumbnail
  by every later run. It is written beside itself and renamed, and a thumbnail
  counts only if it is a whole PNG.
* `media.waveform` wrote atomically but never read its output back. Succeeding
  now means the file was read and holds the channels, rate and length it claims.

### Media too large to hold

Professional media is bigger than memory, so two things are measured rather than
assumed.

**Identifying and probing do not read the file.** `ContentHash::fast` hashes the
size and both ends. Measured: a 536.9 MB file identified in 91.8 ms, with
resident memory growing 1.4 MB (dev build, this machine).

**A picture that will not fit is refused, not attempted.** An EXR's data window
is declared in its header, and a large one — real or corrupt — used to be an
allocation the kernel answered by killing the application, which is not a
failure the application can report or a person can act on. The size is now
checked against a budget before a byte is allocated, in one place
(`cine_media::budget`), and the refusal names the real size, the real budget and
`CINE_PICTURE_BUDGET`, which raises it. The default is half of the memory the
machine reports available, clamped between one 8K float-RGBA picture and 16 GB.
Reading the EXR also builds the frame's bytes directly rather than building
floats and copying them, which halves what one picture costs while it is read.

What is not claimed: this bounds the picture a decoder is asked to produce, not
a decoder's own working memory.

### Source timecode

A frame index counted from zero is this build's name for a frame. The timecode
is everybody else's: an EDL's source-in, a dailies report, a note from the
cutting room, a conform against a cut made in another building. Until now
`MediaFile::start_timecode` was always absent, so every source timecode the
application showed was counted from zero rather than from the camera's clock.

**What a timecode track is.** A real track with a `tmcd` handler, one sample
description and — for a take with continuous timecode — one sample:

```text
flags          bit 0 drop frame, bit 1 24-hour wrap, bit 3 counter not clock
time scale     ticks a second          30000
frame duration ticks a frame            1001   → the rate is 30000/1001, exactly
frames         the labelling rate         30
sample         a 32-bit frame number  107892   → 01:00:00;00
```

The rate comes out of the file as the ratio it is and goes straight into
`Rate`, the same rate the rest of the project counts in. The frame number is
decomposed by `Timecode`, which owns the drop-frame rules. Nothing else does
timecode arithmetic — `Timecode::plus_frames` is the one place a label is
moved, and the interface asks for the answer rather than working it out,
because a second implementation of drop-frame is free to drift from the first.

**Absent is not zero.** A file with no timecode track and a take that starts at
00:00:00:00 are different facts, and every layer keeps them apart: the probe
answers `None`, the media list omits the field, and the inspector says "none in
this file".

**The movie writer writes one**, which is how the tests have real media to read,
and which means a **proxy carries its original's timecode** — a stand-in that
could not be conformed like the thing it stands in for is not a stand-in. A
rendered cut carries none: a sequence has no start timecode in this build yet,
and writing one would invent the number a conform depends on.

**Verified how, and how far.** Round-tripping through this build's own writer
cannot catch a layout mistake the writer and the reader share, so
`timecode_track.rs` also walks the boxes with a second reader written from the
published field offsets, and checks the number in the file against arithmetic
done in the test — one hour of drop-frame 29.97 is `30 × 3600 − 2 × 54 =
107 892`. GStreamer's `qtdemux` reads the rest of the file unaffected but
reports `unknown subtype tmcd`, and no other timecode reader exists on this
machine, so **no independent implementation has confirmed these bytes**. The
capability row says so.

Measured in the real application (`ui/e2e/ingest.mjs`): a `cine bars` roll
written at one hour reads `01:00:00:00` in the inspector, and picture 12 reads
`01:00:00:12` in the viewer.

#### One interpretation, and what it refuses

A timecode track has a start whatever kind of track it is, and the start is
always reported. What is not always allowed is the one operation that turns a
start into a label for an arbitrary frame — adding a frame count to it:

| what the track is | start reported | labels frames | what the EDL does |
|---|---|---|---|
| a continuous clock | yes | yes | source columns in the camera's numbering |
| a **counter** (flag bit 3) | yes | **no** | counts from zero, and says why in the file |
| **discontinuous** (more than one run) | yes, the first run's | **no** | counts from zero, and says why in the file |
| no timecode track | — | — | counts from zero; nothing to say |
| an image sequence | — | — | counts from zero; nothing to say |

`SourceTimecode::labelling` is the only place that decides, and every surface
that turns a picture into a timecode asks it: the viewer's readout, an EDL's
source columns, the timecode track a proxy is written with. A counter counts
takes or feet or edits, so adding frames to it produces a number that looks
exactly like a timecode and is one for nothing. A discontinuous track is right
until the first jam-sync break and wrong after it, which is worse than having
no answer.

**A defect this audit found.** `tmcd.rs` computed both facts and its own
documentation promised a counter would be "reported as a counter rather than
dressed up as a clock" — and then `probe` kept only `.start` and threw the rest
away. Every counter in the project was being presented as a time of day, and
every discontinuous take as a continuous one. `MediaFile` now carries the whole
track, and the inspector says which kind it is and, where frames cannot be
labelled, why not.

#### Conforming: the EDL

The source columns are what a facility looks for on the camera original, so
they are the camera's numbering — the media-relative source range added to the
take's start, never replacing it: the range says which part of the take is
used, and the start says what that part is called. The record columns are the
cut's own numbering, which begins at zero because a sequence has no start
timecode in this build.

Held at 24, 25, 30, 23.976, 29.97 non-drop, 29.97 drop-frame and 59.94, with a
clip starting at a non-zero offset inside a take whose timecode is not zero —
the case where every way of getting this wrong shows up. The labels are read
back and turned into frame numbers rather than compared as strings, because
that is what a conform does with them: the question is not what a label looks
like but which frame it names.

**Two defects this found.**

* `FCM: NON-DROP FRAME` was a constant, printed directly above drop-frame
  labels. Frame Code Mode tells a reader how to interpret every label that
  follows, so a reader honouring it would take `01:00:01;00` for 108 030 frames
  when it means 107 922 — the conform error timecode exists to prevent. The
  mode now follows the rate the cut is at and every label in the file is
  written in it, record columns included.
* `export` exported the *first* sequence in the project rather than the one
  being worked on, for both EDL and OTIO. Nobody notices until a facility cuts
  the wrong reel.

Re-labelling is faithful across namings: a take whose camera wrote non-drop
`01:00:00:00` at 29.97 is frame 108 000, and the EDL writes `01:00:03;18`,
which is the same frame under drop-frame naming, under an FCM line that says
so.

#### What conform does not cover yet

**OTIO export writes an available range starting at zero.** An OTIO reader
takes a media reference's available range as the media's own coordinates, so
that is a claim that the take's timecode starts at 00:00:00:00 — which for a
take the camera numbered `01:00:00:00` is not true. Source ranges are
media-relative everywhere in this build, and the fix is to put them in timecode
space rather than to shift the available range alone, which would break every
trim that clamps against it. Recorded as the limit of `time.timecode.conform`
rather than left to be discovered in a conform.

**A rendered cut carries no record timecode**, because a sequence has no start
timecode here yet. Writing one would invent the number a conform depends on.

### What a job guarantees, audited

Every long thing the application does is a job, so the guarantees below are the
guarantees of all of it. Each was checked against the code, and the third column
names where it is observed rather than asserted.

| property | how it holds | observed in |
|---|---|---|
| persistent identity | an id for the job's whole life; with a journal, every transition is a line of JSON and reopening restores the history | `jobs.rs::history_survives_a_restart_and_interrupted_work_resumes` |
| one job per identity | a spec whose `key` matches a pending or succeeded job returns that job — including when many callers ask at the same instant | `the_same_work_is_not_done_twice`, `the_same_work_asked_for_at_once_by_many_is_still_done_once` |
| truthful progress | reported by the runner, reset to nothing at the start of every attempt, and set to complete only on success — never interpolated, never a timer | `cine-jobs/src/lib.rs` |
| deterministic transitions | priority then submission order among ready jobs of a class; a dependant waits and fails naming its dependency | `higher_priority_runs_first_and_ties_run_in_order`, `a_dependant_waits_and_fails_when_its_dependency_fails` |
| cancellation | cooperative and final: a queued job stops at once, a running one is told, and a runner that ignores it is still recorded as cancelled | `cancelling_a_running_job_stops_it_and_records_it` |
| retry | bounded by `max_attempts`, keeping the last error; a panicking runner fails its job rather than the application | `failures_are_retried_to_the_limit_and_panics_are_contained` |
| stale results | a job cancelled while running is recorded cancelled and its output is **discarded**, so work overtaken by a cancellation cannot be committed | `cine-jobs/src/lib.rs`, `proxy_protocol.rs::a_cancelled_proxy_leaves_nothing_and_no_record_of_itself` |
| atomic derived output | proxy, waveform and render write into `….partial` and rename; the thumbnail writes beside itself and renames. A rename within a directory is atomic; a write is not | `proxy.rs`, `waveform.rs`, `render.rs`, `jobs.rs` |
| no orphaned partials | a partial is temporary data that the job which made it removes, and re-running the same identity clears any left behind. `.partial` directories are skipped when renders are listed, so one can never be shown as a finished render | `provenance.rs`, `STORAGE-POLICY.md` |
| validated completion | succeeding means the output was checked: a proxy is re-probed, a waveform is read back and must hold the channels, rate and length it claims, a render's every frame and its manifest must be present | `proxy.rs`, `waveform.rs`, `provenance.rs` |
| restart and recovery | work that was queued or running when the process stopped is queued again and says so; runners are therefore safe to run from the start, which their keys make explicit | `history_survives_a_restart_and_interrupted_work_resumes` |
| diagnostics | every record carries state, attempts out of the limit, the last error, elapsed time, the resource class, whether it was resumed, and the params it ran with | `jobs.rs::record_json`, the Jobs panel |

What a job does **not** do: checkpoint inside itself (a resumed job starts
over), distribute across machines, or decide what work is needed — the runtime
and the reactors do that.

### Speed

A retimed clip plays its media over a different length of track. The model
holds that as two exact durations rather than a floating-point scalar: 48
frames of media over 24 of timeline is a speed of exactly two, where a
`time_scalar` of 2.0 and one of 1.9999999 put a picture either side of a frame
boundary after a few minutes. The inspector asks for the length for the
same reason — a percentage would have to become a length anyway, and the
rounding would happen where nobody could see it.

Everything downstream carries it. The cut recomposes, because a retime is a
ripple; the clip shows its speed as a badge; the record monitor resolves the
right picture, floored onto the media's own frames because there is no picture
between two pictures; the EDL writes an `M2` motion record with the play rate;
and the OTIO file carries both the exact pair and a `LinearTimeWarp` for other
tools.

A retimed clip can still be cut. The two halves keep their track lengths
exactly and divide the media between them with nothing lost, so together they
consume the same frames over the same time; each half's own speed can differ
from the original by less than one media frame over its length, because the cut
has to land on a picture. A cut so fine that one side would hold no picture at
all is refused rather than made.

A speed can also change over a clip. The inspector's *Speed ramp* takes
segments — a length in frames and the speed at each end, typed as `100%`,
`3/2` or `1.5` and read exactly, never through a float — and the speed moves
linearly across each. A constant speed is one segment with equal ends, so there
is one kind of retime. The ramp decides both lengths: the clip's length on the
track is its segments', and the media it consumes is the integral of the speed,
refused if that runs past the shot. Every player asks the same exact map which
picture or sample plays at an instant — one second into a ramp from real time to
double speed is 1.25 s of media, picture 30 — including the mixer, whose pitch
rises with a varispeed ramp and holds with *Keep pitch*. The EDL can carry only
one speed per event and writes the average, saying so; OTIO carries the ramp
exactly in its metadata and the average for other tools.

What a retimed clip will not do is be trimmed, rolled or joined. Pulling the
tail of a clip playing at 1.3× either keeps the speed — and then needs a
fraction of a media frame — or keeps the media and changes the speed, and an
editor that picks one silently is wrong half the time. Those edits refuse and
say what the two choices are; setting the length again expresses either of them
exactly. Slipping is allowed, because sliding the media under a retime changes
neither the speed nor the length.

## What remains

* **No multi-select, no linked selection, no sync-break indicator.** Edits follow
  links; the interface does not yet draw a broken sync.
* **A cut inside a speed ramp is refused.** A ramp's media boundary at an
  arbitrary instant is generally partway through a picture, which a source range
  cannot hold; cut where a segment ends, or remove the ramp first. Ramps are
  linear in speed between their segment ends — no eased curves.
* **Keeping pitch flams attacks.** A retimed clip's sound is varispeed by
  default — pitch follows speed, as on tape — and *Keep pitch* time-stretches it
  instead. The stretch keeps a tone's frequency and level at any speed, but at
  any speed other than real time a sharp attack is heard twice: a click at ¾
  speed comes out as two copies 8 ms apart. Real time passes it intact.
* **Reverse plays, slowly.** A reversed clip composes, exports and resolves the
  right picture; but each successive picture is *earlier* in the file, so every
  one costs a walk from the preceding keyframe. The scheduler reports that as
  `Degraded` — it is not pretended to be smooth, and it is not pretended to be
  broken either.
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
