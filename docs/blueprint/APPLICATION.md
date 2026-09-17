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
