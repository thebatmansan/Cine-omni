# CINE-OMNI capability matrix

The permanent map of what a filmmaker needs across the whole lifecycle, and where
CINE-OMNI stands on each item. Status is **measured against the code at
`77192a8`**, not intended; see `MASTER-AUDIT.md` for the evidence behind it.

A capability is only as far along as its *whole path*: a decoder with no
interface is IMPLEMENTED, not VERIFIED, because nobody can use it.

## Legend

**Status**

| | Meaning |
|---|---|
| **VERIFIED** | Works end to end on real input, tested, and reachable by a user |
| **IMPLEMENTED** | Real and tested in the engine; not reachable from an interface |
| **PARTIAL** | A real subset works; material parts are missing |
| **PROTOTYPE** | The shape exists; not usable for real work |
| **PLANNED** | Designed in a research dossier; nothing built |
| **MISSING** | Neither designed nor built |
| **BLOCKED** | Needs a dependency, licence or hardware decision first |
| **DEFERRED** | Deliberately postponed; reason given |

**Complexity** S (days) · M (weeks) · L (months) · XL (a team, a year or more).
**Priority** P0 foundation/table stakes now · P1 next wave · P2 needed for a
credible 1.0 · P3 later or specialist.

**Columns.** *Reference* is the professional software or category used today.
*Requires* abbreviates the infrastructure: **BE** backend, **FE** interface,
**M** media, **G** GPU. *Model* is the data model it needs in the UCG.

Research dossier numbers (e.g. `R16`) refer to `docs/blueprint/research/`.

---

## 1. PRE-PRODUCTION

### 1.1 Development and story

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Screenplay import | Final Draft, Fade In, WriterDuet | Lossless FDX/Fountain import | **IMPLEMENTED** | `cine-script` Fountain + FDX | FE: script view | script.* | none | instant to 200 pp | S | P0 | round-trip FDX corpus |
| Screenplay editor | Final Draft 13 | Industry format as you type, element tabbing, smart type, pagination that matches the page count producers budget from | **MISSING** | — | BE paginator · FE editor | script.* | none; do not copy FD UI | keystroke <16 ms at 150 pp | L | P1 | page-count parity vs FD on corpus |
| Revisions | Final Draft revision mode | Colour revision sets, locked pages, A-pages, omitted scenes, collated revision reports — the way a production office expects | **PARTIAL** | issuing pages snapshots the script; every element changed or added since carries a mark, shown in the margin where a printed page has it, and the header says which colour the crew is holding and how many marks stand. The colour sequence is the industry's, and continues as "Second Blue" past the end of it. Editing a line back to what it said removes its mark, because the page that went out is the page that went out. No locked pages, A-pages, omitted-scene handling or revision report; no PDF of revised pages | locked pages · A-pages · omitted scenes · revision report · PDF | script.revision | none | — | M | P1 | `revision.rs`, `script_protocol.rs` |
| Scene numbering & locking | Final Draft | Stable scene numbers after lock (A/B inserts) | **PARTIAL** | numbers imported; no lock | BE · FE | script.scene | — | — | S | P1 | insert between locked scenes gives 12A |
| Outline / beat board | Final Draft Beat Board, Scrivener | Cards linked to scenes, reorder story structure | **MISSING** | — | BE · FE board | story.beat | — | — | M | P2 | beat reorder reflects in scene order |
| Character & location tracking | Final Draft reports | Character/location lists, scene counts, dialogue share | **IMPLEMENTED** | derived in graph | FE report views | script.character/location | — | — | S | P1 | counts vs FD reports |
| Script notes & comments | WriterDuet, Frame.io | Threaded notes anchored to text, resolved state | **MISSING** | — | BE · FE | note (anchored) | — | — | M | P2 | anchor survives edit |
| Script collaboration | Final Draft 13, WriterDuet | Real-time co-writing | **PARTIAL** | runtime shares state live; no text CRDT | BE text merge · auth | script text ops | — | <100 ms remote echo | L | P2 | two writers, no lost keystrokes |
| Page / scene timing | Final Draft, breakdown tools | Eighths, estimated screen time | **IMPLEMENTED** | eighths, est. duration | FE | script.scene | — | — | S | P0 | eighths vs FD |
| Script export (FDX/PDF) | Final Draft | Studio-accepted FDX and PDF | **PARTIAL** | FDX/Fountain export; no PDF | BE PDF typesetter | — | font licensing (Courier Prime is OFL) | — | M | P1 | PDF page breaks match FDX pagination |

### 1.2 Breakdown, scheduling and budget

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Script breakdown & tagging | Movie Magic Scheduling, StudioBinder | One breakdown sheet per scene; tagged cast, props, wardrobe, vehicles, SFX, VFX, animals, extras | **PARTIAL** | breakdown derived from the script, and correctable by hand: cast and props added or struck, the vehicles/animals/SFX/VFX flags overruled or handed back to the script, in the Production workspace and through `tag_breakdown`. Tags live on the scene, not in the derived sheet, so a rewrite re-derives around them — tested. Each element says whether the script said it or a person did. No wardrobe, make-up, stunts, or custom categories | more categories · custom categories · element library | logistics.breakdown | — | — | M | P1 | `production_protocol.rs` (tag survives a rewrite) |
| Stripboard | Movie Magic Scheduling | Drag strips to reorder shooting order, day breaks, colour by INT/EXT/DAY/NIGHT | **PARTIAL** | Production workspace: auto-scheduled board in industry colours, pages in eighths, constraints editable and rescheduling live; a script rewrite reaches the board (measured 10.6 ms cascade); a strip can be moved to another day and stays there — the pin is a constraint, so rescheduling keeps it, undo releases it, and a pin that overfills a day is honoured and warned about (measured 3.5 ms). Moved with buttons, **not yet dragged** | FE drag · day breaks · banners | logistics.schedule/day | — | 300 strips drag at 60 fps | M | P1 | `schedule.rs` (pins), `production_protocol.rs`, `ui/e2e/production.mjs` |
| Day out of days | Movie Magic Scheduling | Per-cast work/hold/travel/drop-pickup days | **PARTIAL** | derived from the stripboard: SW/W/H/WF/SWF per cast per day, work and hold totals, in the Production workspace and as `cine export <dir> dood`; hold days are counted separately, since re-ordering the board moves them and not the work. No travel, drop/pickup or per-cast rate arithmetic | travel · drop/pickup · contracts | logistics.day + cast | — | — | S | P1 | `dood.rs` (codes and hold days), `production_protocol.rs` |
| Call sheets | StudioBinder, MMS | Daily call sheet from schedule, crew, locations, weather, hospitals | **PARTIAL** | derived per day and shown in the Production workspace; regenerated on reschedule and on script change (tested); printable as a PDF from the panel, the protocol (`/callsheet?day=N`) or `cine export <dir> callsheet-pdf` — written by a small PDF writer in this workspace (no dependency), read back by poppler and parsed by Ghostscript in the tests. The boxes with nothing behind them (weather, hospital, unit base, transport) say so on the page | real crew data · weather · hospitals · distribution | logistics.call_sheet | — | 3.1 kB, one page | M | P1 | `callsheet_pdf.rs` (pdftotext + gs), `production_protocol.rs` |
| Budgeting | Movie Magic Budgeting, Hot Budget | Chart of accounts, fringes, globals, schedule-linked labour | **MISSING** | — | BE budget model · FE | budget.* | union rate tables are proprietary data | — | L | P2 | schedule change moves labour lines |
| Crew & cast management | StudioBinder | Contacts, roles, availability, deal memos | **MISSING** | cast/crew kinds exist | BE · FE · privacy | logistics.crew/cast | personal data (GDPR) | — | M | P2 | availability conflict flagged |
| Equipment planning | Rental house tools | Camera/lens/grip packages per day | **MISSING** | lens grid kind exists | BE · FE | logistics.lens_grid | — | — | M | P3 | — |
| Location management | Reggie, StudioBinder | Scouting photos, permits, contacts, maps | **MISSING** | script.location only | BE · FE · M images | location.* | map tiles licensing | — | M | P2 | — |
| Continuity (costume, hair, makeup, props) | Script supervisor apps, ScriptE | Per-scene look tracking with photos, day-to-day continuity | **MISSING** | — | BE · FE · M images | continuity.* | — | — | M | P2 | — |

### 1.3 Visual development: boards, shot design, previs

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Storyboarding | Storyboard Pro, Storyboarder, pen & paper | Fast drawing, panels linked to scenes/shots, dialogue/action notes, camera moves | **PROTOTYPE** | board.frame/layer/stroke kinds; no drawing | FE canvas · BE strokes | board.* | Storyboarder is MIT "with exceptions" — review before borrowing | pen latency <20 ms | L | P1 | panel ↔ shot link survives script change |
| Animatics | Storyboard Pro, Premiere | Boards on a timeline with timing and scratch audio | **MISSING** | timeline exists | FE · BE board clips | nle.clip → board.frame | — | — | M | P1 | animatic duration ↔ scene timing |
| Shot list | StudioBinder, Shot Designer | Shot size, angle, movement, lens, notes, per scene | **PARTIAL** | shot list derived from previs cameras | FE editable list | logistics.shot_list/shot | — | — | S | P0 | edit shot → schedule |
| Shot design / floor plans | Shot Designer, Cine Tracer | Top-down camera and blocking diagrams | **MISSING** | — | FE 2D canvas · BE | previs.stage | — | — | M | P2 | — |
| Previs (3D blocking, cameras, lenses) | Cine Tracer (Unreal), FrameForge, Maya | Real lens FOV, sensor sizes, blocking, camera moves, export to shots | **PROTOTYPE** | stages, cameras, USDA-shaped export; no viewport | FE 3D viewport · G raster | previs.* | OpenUSD licence review (R17) | 60 fps viewport | XL | P2 | FOV vs lens calculator |
| Techvis / stuntvis | Proprietary previs houses | Real-world measurements, rigs, speeds | **MISSING** | — | BE · FE 3D | — | — | — | XL | P3 | — |
| VFX breakdown & bidding | ShotGrid/Flow, spreadsheets | Per-shot VFX tasks, complexity, cost | **MISSING** | — | BE · FE | vfx.task | — | — | M | P2 | — |
| Production design / art | Pinterest, Milanote, SketchUp | Reference boards, set plans linked to scenes | **MISSING** | asset.model kind | FE board · M images | asset.* | — | — | M | P3 | — |

---

## 2. PRODUCTION

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Card offload with verification | Silverstack, ShotPut Pro, Hedge | Copy to ≥2 destinations, verify, never mark verified what was not | **PARTIAL** | `cine offload <card> <destination>`: every file read once and hashed as it is read, written, flushed and synced, then read **back from disk** and hashed again, with an ASC MHL generation written into the destination; a file already at the destination is refused, never overwritten. One destination, no resume, no interface — command line only | second destination · resume · job kind · FE queue | offload.job | — | measured on this machine in the test | M | P1 | bit-flip detection (`cine-media/tests/mhl.rs`) |
| Checksum manifests | ASC MHL | ASC MHL v2 manifests, xxh64/xxh128, chain of custody | **PARTIAL** | ASC MHL v2 generations written and read (`xxh64`, `xxh3`, `md5`, `sha1`, hashes checked against published vectors); `cine verify <folder>` checks a folder against its newest list and records the check as the next generation, so the history is the folder's own; a changed byte is reported with both hashes, a missing file and an unlisted file as different things. Whole-file SHA-256 as a job remains. No chain file, directory hashes, child histories or ignore patterns — each named in `unimplemented` when a list uses them | chain file · directory hashes · nested histories | offload.manifest | ASC MHL is BSD-style (verify) | — | S | P1 | `cine-media/tests/mhl.rs`; not yet validated against the reference `ascmhl` tool |
| Media registration & identity | Every NLE | Content identity, relink after move, offline as a state | **VERIFIED** | content hash, relink, online flag | — | asset.media | — | — | — | P0 | relink test; visible in browser |
| Camera & lens metadata | Silverstack, ARRI/RED tools | Camera, lens, focal length, T-stop, focus distance, per frame where available | **PARTIAL** | kinds modelled (R15) | BE parsers · M | media metadata | RAW SDK terms (below) | — | L | P1 | round-trip vs camera reports |
| Takes, slates, circle takes | Script supervisor, Moviola | Take status vocabulary (not boolean), slate identity, circled takes | **PARTIAL** | takes bind to media | FE · BE status vocab | logistics.take | — | — | M | P1 | take survives shot regeneration (tested) |
| Multi-device take reconciliation | Silverstack, Resolve | Camera A/B + sound recorder merged into one take by TC/slate | **PLANNED** | match-key cascade (R15) | BE | take ↔ media[] | — | — | M | P1 | reconcile fixture of 5 streams |
| Sound reports & poly WAV | Sound Devices, Zaxcom | iXML/BEXT metadata, track names, scene/take | **MISSING** | WAV decode only | BE parser · M | audio metadata | — | — | S | P1 | iXML fixtures |
| Timecode & sync | Tentacle, Ambient | Jam-synced TC, audio sync by TC or waveform | **PARTIAL** | waveform sync (`cine-timeline::sync`), TC parse | FE · BE | sync.alignment | — | — | M | P1 | sync fixture ±1 frame |
| Dailies | Resolve, Colorfront, Pix | Graded, synced, watermarked review media, per-day | **MISSING** | — | BE render · M encode · G | dailies.* | encoder licensing | faster than real time | L | P1 | — |
| Media relink | Resolve, Premiere, Avid | Relink by content, not by name; ambiguity surfaced | **IMPLEMENTED** | `media.relink` job scans a folder and matches by content hash (full SHA-256 when the project has a verified one, recorded as `exact` rather than `fingerprint`); moved and renamed media is found and the asset keeps its identity, so clips, proxies, grades and renders still refer to it; a namesake with other content is refused by scan and by hand with both hashes; duplicates reported, not chosen; unresolved state and reason kept on the asset and shown in the inspector | relink across projects · replace-media flow · partial-card scanning | `media.relink`, `relink` RPC, `relink_state` | — | scan hashes only files whose size or name matches | M | P1 | `relink_protocol.rs` (7 tests), `ui/e2e/relink.mjs` |
| Proxy generation | Resolve, Premiere | Background proxies tied to originals, never promoted to source | **IMPLEMENTED** (picture + ≤2ch sound, H.264/PCM in QuickTime) | `media.proxy` job: decode → scale (Lanczos-3) → re-encode, the original's frame count and rate picture for picture, its sound sample for sample when it has one or two channels (more have no downmix: the proxy carries none and says so). Registered as a `media.derivative` with a `DerivedFrom` edge, never an asset — so no clip can bind to it and no render can read it. Records source hash, recipe (codec, size, quality, recipe version), its own hash, size, probed streams, notes, job. `current`/`stale`/`missing` recomputed on request; cancellable; a refused or cancelled proxy leaves nothing. Inspector shows and makes them | >2ch downmix · other codecs · GPU scaling · automatic generation on ingest | `media.derivative`, `media.proxy` job | H.264 via the build's existing `rusty_h264`; no new dependency | 640×360→320×180 4.4 pictures/s; 1920×1082→958×540 1.9 pictures/s (dev, CPU; stages measured) | M | P1 | `proxy_protocol.rs` (8 tests: picture/sound identity, staleness, refusals, cancel, retry), `ui/e2e/proxy.mjs` |
| On-set LUT / live grade | Pomfort Livegrade, Resolve | CDL/LUT per camera, applied live, travels to post | **PARTIAL** | CDL, LUT engine | FE · live input | color.grade | — | real time | M | P2 | CDL export matches ASC spec |
| Live monitoring | Resolve Live, SDI tools | SDI/NDI/HDMI input at low latency | **BLOCKED** | — | BE capture · G | — | capture card SDKs are proprietary | <2 frames | L | P3 | — |
| Virtual production / LED volumes | Unreal nDisplay, disguise | Real-time engine, camera tracking, frustum rendering | **DEFERRED** | — | separate process (Unreal) | — | Unreal EULA: linear content royalty-free; engine not embeddable | — | XL | P3 | — |
| Camera & lens tracking | Mo-Sys, StYpe, FreeD | Tracking data recorded with takes | **MISSING** | — | BE protocol · M | per-frame metadata | FreeD is an open protocol | — | M | P3 | — |
| Motion capture ingest | Vicon, OptiTrack | FBX/BVH ingest, retargeting | **DEFERRED** | — | BE · 3D | — | FBX SDK proprietary | — | L | P3 | — |
| Production notes & reports | ScriptE, spreadsheets | Daily progress, script supervisor notes linked to takes | **MISSING** | — | BE · FE | note on take | — | — | S | P2 | — |
| Backup & LTO archive | YoYotta, Hedge | LTFS writes with manifests and catalogue | **MISSING** | — | BE · tape hardware | archive.* | — | — | L | P3 | — |

---

## 3. POST-PRODUCTION

### 3.1 Editorial

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Timeline view | Resolve, Premiere, Avid | Tracks, clips, transitions, markers, ruler, playhead, zoom | **VERIFIED** | browser + native, from graph | — | nle.* | — | 60 fps redraw | — | P0 | projection tests + e2e |
| Track state: lock/mute/solo/target | All NLEs | Exactly these, with solo ≠ mute | **VERIFIED** | commits through graph, live across clients | — | OTIO metadata | — | — | — | P0 | 19 tests + live-sync test |
| Insert / overwrite | All NLEs | Source → record with targeting | **VERIFIED** | `EditOp` + `place` (picture and sound placed linked, to targeted tracks); UI `,` / `.` from marked source; binds the project's asset, never a second identity | — | nle.* | — | commit 1.7 ms median (e2e) | S | **P0** | `editorial_protocol.rs`, `ui/e2e/editor.mjs` |
| Three-point / four-point editing | Avid, Premiere | In/out on source and record, fit-to-fill | **VERIFIED** | `three_point` resolves any three of four marks and returns the plan: derived points named, backtimed edits flagged, checked against the asset's available range before committing; `I`/`O` mark whichever monitor is active and the ruler draws the band; four marks that disagree are refused with the difference in frames unless the editor picks how to settle them — trim head, trim tail, keep source length, or fit to fill, which sets an exact retime | — | marks are editor state | — | — | M | **P0** | 12 unit + 5 graph + 2 protocol tests; `edit.test.ts` |
| Ripple / roll / slip / slide / trim | All NLEs | Handle-bounded, reports applied delta, mixed rate | **VERIFIED** | drag tools with frame readout; runtime clamps and reports requested vs applied; one undo step | — | nle.* | — | edit → other client refetched 5.3 ms median | M | **P0** | protocol tests incl. clamping; UI gesture tests; e2e |
| Split / join / lift / extract / replace | All NLEs | — | **VERIFIED** (split, lift, extract) / **IMPLEMENTED** (join, replace) | razor / ⌘K, ⌫, ⇧⌫ in UI; join and replace reachable by protocol only | FE join/replace gestures | nle.* | — | — | S | **P0** | `linked_edits.rs`, e2e |
| Snapping | All NLEs | Cuts, markers, playhead; zoom-independent | **PARTIAL** | UI snaps drags to runtime-composed cut points and the playhead (8 px); markers not targets; engine `snap` unused | FE marker targets | — | — | — | S | P0 | `edit.test.ts` |
| Linked A/V | All NLEs | Linked selection and edits, sync-break indicators | **PARTIAL** | edits follow link groups in one transaction; the group agrees on one clamped delta; locked linked track refuses; **no sync-break indicator**, no linked selection | FE indicator | OTIO metadata | — | — | M | P0 | `linked_edits.rs`, protocol |
| Multi-selection & track targeting | All NLEs | Range and multi-clip selection | **MISSING** | — | FE · BE multi-op tx | selection (session, not project) | — | — | M | P0 | — |
| Markers | All NLEs | Clip and timeline markers with colour/notes | **PARTIAL** | modelled + projected; not editable | RPC · FE | nle.marker | — | — | S | P1 | — |
| Transitions | All NLEs | Dissolves, wipes, handle check | **PARTIAL** | modelled; **no handle check** | BE check · render | nle.transition | — | — | M | P1 | unrenderable dissolve refused |
| Multicam — angles, sync and switching | Resolve, Premiere, Avid | Sync by TC/audio, angle switching live | **IMPLEMENTED** (angles, offsets, sync by sound, four-up stepping view, switching) / **MISSING** (live four-angle playback, timecode sync) | an angle is one of the clip's media references, every angle's asset bound to the clip; per-angle offsets against the take carry a switch onto the same moment in the other camera's media; *Sync by sound* is a job that decodes each angle, cross-correlates it with the live angle and accepts only when the peak and the loudness agreement both pass (measured: one take 0.9999, twelve strangers ≤ 0.43); the record monitor's Angles mode decodes up to four angles at the playhead, says why an angle has no picture, and cuts on click | live playback needs four decoders in the native player; sync by timecode | reference keys, `nle.angles`, `cine_angle_offsets` | — | — | L | P1 | 7 model/graph + 2 real-WAV sync + 1 protocol test |
| Nested / compound sequences | All NLEs | Sequences as clips | **MISSING** | OTIO stacks exist | BE · FE | nle.stack | — | — | M | P1 | — |
| Keyframes & effects | All NLEs | Opacity, position, speed, audio levels; curves | **MISSING** | — | BE · FE · G | animated params | — | — | L | P1 | — |
| Retiming — constant speed | All NLEs | Speed changes, freeze frames, reverse | **VERIFIED** (constant) / **MISSING** (ramps) | a retime is an exact pair — the media consumed and the track time it takes — never a float scalar; composition, the OTIO file (with `LinearTimeWarp` for other tools), the graph, the flattened picture list, `picture_at`, the EDL's `M2` record and fit-to-fill all carry it; trims/rolls/joins refuse a retimed clip rather than guess; reverse resolves the right picture and decodes slowly (reported Degraded). Retimed **sound** plays as varispeed through the mixer's exact-position band-limited kernel, forwards and backwards, or — with *Keep pitch* — time-stretched by a memoryless phase vocoder that keeps frequency and level and seeks consistently; files at other sample rates are converted the same way | speed ramps; attacks flam when pitch is kept | `nle.retime_played`, `nle.retime_keep_pitch` | — | varispeed 74–81x, pitch kept 10–14x real time (dev profile) | M | P2 | 13 retime + 1 protocol + 8 real-WAV mixer + 6 kernel + 5 stretch tests |
| Retiming — speed ramps | All NLEs | Retime as a function, not a scalar | **IMPLEMENTED** | a retime is an exact time map of linear-speed segments (`ucg_core::TimeMap`, `i128` rationals); a constant speed is one segment, so every player — picture list, record monitor, native player, mixer, stretch — reads one map. The inspector takes segments with speeds typed exactly (`150%`, `3/2`); the ramp decides the track length and media consumed and is refused past the shot; OTIO carries it exactly, the EDL its average with a note | eased curves; a cut inside a ramp is refused, not approximated | `nle.retime_ramp` | — | — | M | P2 | 5 map + 4 model/graph + 2 real-WAV mixer + 1 protocol test |
| Captions & subtitles | Resolve, Premiere, subtitle tools | SRT/TTML/IMSC/SCC authoring, burn-in | **IMPLEMENTED** (authoring, import, export incl. SCC, script link, timeline lane, burn-in in a render) | a Captions tab per sequence and language: add at the record marks or the playhead, edit text in place, move to the top, remove; import SRT with what moved onto a frame reported; export IMSC, SRT or WebVTT. Every change is one commit shared across windows and undoable; each caption shows the runtime's delivery issues. Stored in the project, not under the timeline, so re-importing the cut does not delete them. Linked to the dialogue they transcribe through the cut's scene bindings; a rewrite, a cut line, a removed scene or a re-cut flags the caption with the reason, a moved line is followed, and the script shows the link. Burn-in: a render chooses a language and the captions are drawn on the IMSC grid the exported file declares (32×15 cells, 10% safe area), from a system font only; a line too wide for the safe area refuses the render rather than shrinking | roll-up SCC · burn-in styling beyond the delivered layout | `nle.captions`, `nle.caption`, `DerivedFrom` to `script.dialogue` | — | review of 1,200 linked captions 49 ms | M | P1 | 5 graph + 8 pipeline + 2 protocol + 3 browser tests; burn-in 5 compositor + `render_protocol.rs` |
| Proxy editing & conform | All NLEs | Cut on proxies, conform to originals exactly | **PARTIAL** | a session-wide choice (Originals / Proxies / Auto) decides per clip what every surface reads — record monitor, native window, edit list — while renders always read originals; the picture index is the same in either file, so no conform step is needed; a stale or missing proxy is never read and the reason is shown | offline-only conform reports · relink after replacing originals | session media preference, `picture_at.proxy` | — | — | M | P1 | `proxy_protocol.rs`, `ui/e2e/proxy.mjs` |
| Relink / offline media | All NLEs | Relink by content, name, TC | **VERIFIED** (engine) / **IMPLEMENTED** (UI) | content-hash relink | FE relink dialog | asset.media | — | — | S | P1 | relink tests |
| OTIO interchange | OpenTimelineIO | Read and write real OTIO | **PARTIAL** | write Tier 1; read fails real files | BE reader hardening | nle.* | Apache-2.0 spec; no usable Rust binding | — | M | P1 | 5-tier harness (R16) |
| EDL | CMX3600 | EDL import/export | **PARTIAL** | export: one track as CMX3600 (`cine export <dir> edl [--scene track]`, or through the protocol) with source and record timecode, reel names in the eight-character column and full names in `FROM CLIP NAME`, gaps advancing the record clock as black. What the format cannot carry — other video tracks, dissolves, clips with no source range — is written into the file as `* NOT IN THIS EDL:` rather than left for the receiver to discover. No import, no drop-frame, no dissolves, no audio levels | import · drop-frame · dissolves | nle.* | — | — | S | P2 | `edl.rs`, `editorial_protocol.rs`; round-trip vs Resolve still owed |
| AAF | Avid | AAF import/export (Avid and Pro Tools) | **BLOCKED** | — | BE CFB reader/writer | nle.* | **LibAAF is GPL — must not be read**; AAF SDK under its own public source licence (review); `cfb` crate MIT | — | L | P1 | Avid/Pro Tools round-trip |
| FCPXML / Premiere XML | Final Cut, Premiere | Import/export | **MISSING** | — | BE | nle.* | — | — | M | P2 | — |
| Render cache | Resolve, Premiere | Background renders, invalidated precisely | **MISSING** | UCG dirty propagation exists | BE cache · jobs · storage policy | cache entries | — | bounded by budget | L | P1 | edit invalidates only affected range |
| Timeline versions | Resolve, Avid bins | Duplicate, compare, restore sequences | **PARTIAL** | op log + undo | FE · BE version snapshots | timeline version | — | — | M | P2 | — |
| Collaborative editing | Resolve Blackmagic Cloud, Avid Nexis | Bin locking, shared projects | **PARTIAL** | one runtime, live state; no auth, no locking | BE auth · locks | permissions | — | — | L | P2 | two editors, no lost edits |

### 3.2 Media engine

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| H.264 decode | FFmpeg | 8/10-bit, 4:2:0/4:2:2, all profiles | **PARTIAL** | High 8-bit 4:2:0 pure Rust | M | — | **patent pools** (Via LA / Access Advance); decoder licence obligations depend on distribution | ≥ real time at 4K | M | P0 | conformance bitstreams |
| HEVC decode | FFmpeg, hardware | 10-bit HDR, 4:2:2 | **BLOCKED** | refused | M · G hw decode | — | patents; no credible pure-Rust decoder | 4K real time | L | P1 | — |
| ProRes decode | Apple / FFmpeg | All flavours | **BLOCKED** | refused | M | — | FFmpeg LGPL build only (R02); Apple licensing for encode | — | M | P1 | — |
| DNxHR decode | Avid / FFmpeg | — | **BLOCKED** | refused | M | — | FFmpeg LGPL | — | M | P2 | — |
| Camera RAW (BRAW, R3D, ARRIRAW, CinemaDNG) | vendor SDKs | Debayer with metadata-driven colour | **BLOCKED** | — | separate process or dynamic SDK | — | BRAW SDK free; R3D SDK redistributable only unmodified as part of a larger product; ARRI SDK terms unverified | — | L | P2 | vendor sample clips |
| EXR / image sequences | OpenImageIO, OpenEXR | Multipart, deep, data windows, AOVs | **PARTIAL** | compositor: read and write with both windows, layers by name, `%04d`/`####` sequences, matching Blender's read exactly. **Media**: a numbered run of EXR/PNG/TIFF/JPEG is one asset with the rate it was ingested at, its identity the run itself; EXR read as 32-bit float unclamped (scene values above 1 and below 0 survive), 16-bit stills as 16-bit; holes are part of the run and refuse by name; it cuts, identifies and previews (clamped, and says so) | deep EXR · multipart writing · tiled/streaming reads · delivering scene-linear output · relink scanning for runs | comp read/write, `media.sequence`, `Pictures` | `exr` BSD-3-Clause (`licenses/THIRD-PARTY.toml`) | whole-file reads per frame | M | **P0** (blocks VFX) | `exr_oracle.rs`, `exr_graph.rs`, `image_sequence.rs`, `sequence_protocol.rs` |
| Audio decode | FFmpeg, symphonia | WAV/BWF, AIFF, AAC, MP3, FLAC | **PARTIAL** | symphonia (MPL-2.0) | M | — | MPL-2.0 file-level: fine unmodified | — | S | P0 | — |
| Colour metadata parsing | FFmpeg | `colr`, VUI, mastering display | **PARTIAL** | H.264 SPS/VUI parsed (clean-room from H.264 §7.3.2.1.1, Annex E); pictures carry what the stream declares, unspecified stays absent; `colr` box and mastering-display SEI not parsed | `colr` · SEI | media colour meta | — | — | S | **P0** | parsed size = decoded size on real streams (`sps_real.rs`) |
| Hardware decode | NVDEC, VA-API, VideoToolbox | Where available, with software fallback | **MISSING** | none claimed | M · G · OS APIs | capability registry | vendor APIs | — | L | P1 | parity vs software decode |
| Playback scheduler & decode-ahead | Every NLE | Real time, dropped-frame honesty | **VERIFIED** (native) | 144/144 at 24 fps | — | — | — | — | — | P0 | 60 tests + measured |
| Multiresolution / proxies in playback | Resolve | Choose decode resolution adaptively | **PARTIAL** | Auto reads a current proxy when the original is offline, undecodable by this build, or larger than HD; otherwise the original. The decision is one function every surface calls, and it reports why | per-machine capability measurement · adapting while playing · partial/tiled decode | `proxy::choose` | — | — | M | P1 | `proxy_protocol.rs` |
| Frame cache & render cache hierarchy | All | RAM → disk → proxy, evicted by budget | **PARTIAL** | RAM frame cache, decode-ahead by bytes | BE disk cache | cache | storage policy | bounded | M | P1 | eviction under budget |

### 3.3 VFX and compositing

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Node graph | Nuke, Fusion | Typed nodes, caching, pull evaluation | **PROTOTYPE** | 28 node kinds, persistence, data windows throughout: merges produce the union of their inputs' windows, transforms their transformed window, and a region of interest is pulled up the chain (a 64² region of a blurred 2048² frame in 16.7 ms against 4.5 s for the frame) | FE node editor · file I/O | comp.graph/node | — | interactive on 2K | L | P1 | `evaluator.rs`, `nodes.rs` |
| Image I/O on plates | Nuke | Read/Write EXR, DPX, TIFF, sequences | **MISSING** | re-verified: no file I/O | M | — | OpenEXR, OIIO | streaming | M | **P0** | — |
| Data windows / ROI | Nuke bbox | Work only where pixels exist | **PARTIAL** | `Image` carries its canvas window; generators and passes allocate only the region; every node's result cropped to its region; regions of real chains equal the full frame (edges included). Merge output takes B's window (no union bbox); transform, roto and procedural still compute the full frame and crop | union bbox · windowed transform | image | — | 64² of a 2048² blur: 16.7 ms / 16,384 floats vs 4.5 s / 16.8 M | M | **P0** | `evaluator.rs` region tests |
| Layers / precomps | After Effects | Layer-based compositing | **MISSING** | — | FE · BE | — | — | — | L | P2 | — |
| Merge / premult correctness | Nuke | Correct over/add/premult handling | **PARTIAL** | additive-element defect found (R12) | BE | — | — | — | S | P1 | additive glow survives grade |
| Keying | Nuke, Keylight | Chroma/luma/difference keys | **PARTIAL** | keyer kernel (GPU) | FE · real plates | — | — | real time 2K | M | P1 | — |
| Rotoscoping | Nuke Roto, Silhouette | Splines, feather, tracking-linked shapes | **MISSING** | — | BE · FE spline editor | roto shapes | — | — | L | P1 | — |
| 2D / planar tracking | Nuke, Mocha | Point and planar tracking | **MISSING** | — | BE · G | tracks | libmv is MIT (R01) | — | L | P1 | synthetic ground truth |
| Camera tracking / matchmove | Nuke, SynthEyes, PFTrack | Solve camera + lens distortion | **MISSING** | — | BE | camera solve | libmv MIT | — | XL | P2 | — |
| Paint / cleanup | Nuke, Silhouette | Clone, reveal, wire removal | **MISSING** | — | FE · G | — | — | — | L | P2 | — |
| Optical flow / retiming | Nuke, Twixtor | Motion vectors | **MISSING** | — | G | — | — | — | L | P2 | — |
| Deep compositing | Nuke | Deep EXR, correct alpha maths | **PARTIAL** | linear alpha scale — **wrong** (Beer-Lambert required) | BE | — | OpenEXR | — | M | P3 | OpenEXR deep spec |
| Expressions | Nuke, AE | Parameter expressions | **MISSING** | — | BE language | — | — | — | M | P2 | — |
| OpenFX plugin hosting | Nuke, Resolve, Fusion | Run third-party OFX plugins | **PLANNED** | R12 | BE host · FFI | — | **OpenFX is BSD-3-Clause** (verified) | — | L | P2 | load a reference OFX plugin |

### 3.4 Colour

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Display transforms | Resolve, Baselight | Correct, declared, never substituted | **VERIFIED** | ACES 2.0 output transform ported from the Academy's aces-core CTL (`aces2.rs`): all 31 displays OpenColorIO 2.5.2 registers, both directions, 3,038 samples, worst forward 1.8e-5 / inverse 5.2e-4; ACES 1.x refused. Applied in the native window — each clip's input colorspace through the chosen display and view, baked to a 33³ lattice, interpolated in the fragment shader; the view and the lattice's distance from the transform are shown on the picture. Choosing a view in the browser reaches every surface | grading · scopes · an analytic GPU transform instead of a lattice | color.config | aces-core Apache-2.0, config BSD-3 (`licenses/THIRD-PARTY.toml`) | lattice 88 ms to bake, 0.6 MB; draw incl. conversion 11.6 ms over 55 draws (dev build, Intel UHD) | M | P1 | `aces2_oracle.rs`, `planar_present.rs`, `view_lattice.rs`, `ui/e2e/colour.mjs` |
| OCIO config loading | OpenColorIO | Load studio and ACES configs | **PARTIAL** | the cg and studio ACES 2.0 configs OCIO 2.5.2 ships load (shared views, `<USE_DISPLAY_NAME>`); file rules decide a file's colorspace as OCIO decides it (42 paths, every rule kind); remaining unimplemented keys listed (viewing rules, named transforms, environment, virtual display, inactive colorspaces); compared with OpenColorIO on the same files: 176 source/display/view combinations render as OCIO does (worst code value 1.5e-5), video into an ACES view included; 158/160 colorspace conversions match (worst 1.6e-5), ACES→ADX refused and ACEScc below zero kept to the Academy reference (adjudicated). The cg config is bundled as every new project's config | viewing rules · named transforms · environment | color.config | OCIO BSD-3-Clause (verified); configs used unmodified as test data | — | M | **P0** (for colour) | `released_configs.rs`, `display_styles.rs`, `aces2_oracle.rs`, `file_rules.rs` |
| Primaries: wheels, lift/gamma/gain, offset | Resolve | Precise, GPU, per-shot | **PARTIAL** | ASC CDL per clip — slope, offset, power, saturation — set from the Colour workspace or `set_grade`, applied in the working space and baked into the lattice the native window already interpolates, so grading and viewing are one chain. Measured: a warm grade moves mid grey from 0.359 to 0.421/0.356/0.313, and the graded lattice stays 1.6 of 255 from its own transform. No wheels, no lift/gamma/gain controls, no node graph | wheels · LGG · nodes · GPU-side grade | color.grade | — | rebakes in ~90 ms per graded clip | M | P1 | `view_lattice.rs`, `colour_protocol.rs` |
| Curves | Resolve | RGB, hue-vs-X | **MISSING** | — | FE · G | grade | — | — | M | P1 | — |
| Qualifiers & windows | Resolve | HSL keys, power windows, tracking | **MISSING** | keyer exists | FE · G · tracking | grade nodes | — | — | L | P1 | — |
| Node-based grading | Resolve | Serial/parallel/layer nodes | **MISSING** | comp graph could host | FE · G | grade graph | — | — | L | P2 | — |
| LUTs & CDL | Resolve | 1D/3D LUT, .cube, CDL export | **IMPLEMENTED** | LUT, CDL | FE | color.lut/cdl | — | — | S | P1 | .cube round-trip |
| Scopes (waveform, parade, vectorscope, histogram) | Resolve | Accurate, real time, HDR-aware | **PARTIAL** | waveform and vectorscope in the native window, measured from the picture *as shown* — the same YUV conversion and the same view lattice the shader uses — on a sixteenth of the pixels, while the panels redraw rather than per picture; the pane says how many samples and what they cost. Parade, histogram and HDR scopes exist in `cine-color` but are not drawn; none in the browser | parade/histogram/HDR panes · browser scopes · full-resolution or GPU measurement | — | — | measured per redraw, not per picture | M | **P0** (for colour) | known-signal scope tests (`cine-shell/src/scopes.rs`) |
| HDR (PQ/HLG) grading & monitoring | Resolve Studio | HDR scopes, mastering metadata | **MISSING** | — | G HDR surface · BE | mastering meta | — | — | L | P2 | — |
| Dolby Vision | Resolve Studio | Analysis metadata; trims | **BLOCKED** | — | Dolby tools | — | Dolby: basic metadata free in tools; trims licence $2,500/yr; SDK terms proprietary | — | L | P3 | — |
| Shot matching, stills/gallery | Resolve | Reference stills, split-screen compare | **MISSING** | — | FE · M | stills | — | — | M | P2 | — |
| Camera RAW / log colour science | Resolve | Log curves, gamut per camera | **PARTIAL** | log/gamut → ACES for ARRI LogC3/C4, Sony S-Log3, RED Log3G10, Panasonic V-Log, Canon Log 2/3 (CAT02), Apple Log, ADX10/16 — each matches OCIO through the released configs; no RAW | BE · RAW SDKs | — | vendor specs | — | M | P2 | vendor LUT parity |

### 3.5 Audio

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Device output | Pro Tools, Fairlight | Low-latency playback | **VERIFIED** | `cine-audio-io`: ALSA via runtime-loaded libasound (PipeWire default device); 48 kHz within 61 ppm of the system clock, 71 ms max device delay, no xruns | lower-latency buffers; device selection UI | — | LGPL system library loaded at runtime, nothing linked or redistributed | 50 ms requested buffer; <10 ms not yet attempted | M | **P0** | `cine-audio-io/tests/device.rs` |
| A/V sync | All NLEs | Picture paced by sound, offset measured | **VERIFIED** | native window plays the sequence's mixed sound on the device and paces the picture from it: 70/70 pictures, 0 dropped, 0 audio underruns; picture minus sound −3.5 ms median, 5–95% −4.6..−1.5 ms (device-reported delay, not acoustic) | browser A/V; acoustic loopback measurement | — | — | ±1 ms target not met: ~3.5 ms bias | S | P0 | `--window-play` evidence run |
| Multitrack editing | Pro Tools | Clip gain, fades, crossfades, sample-accurate edits | **PARTIAL** | sample-accurate splices, fades | FE · RT engine | audio.clip | — | — | L | P1 | — |
| Real-time mixer, buses, sends | Pro Tools, Fairlight | Real-time-safe graph | **PARTIAL** | offline renderer, compiled; 48 stereo fine; 128×7.1.4 misses | BE streaming engine | audio.bus | — | deadline at 128 tracks | L | P1 | block deadline histogram |
| Automation | Pro Tools | Volume, pan, plugin params, sample accurate | **MISSING** | — | BE · FE | automation lanes | — | — | M | P1 | — |
| EQ, dynamics, DSP | Pro Tools | Pro-grade processing | **PARTIAL** | biquads, compressor | BE | — | — | — | M | P1 | — |
| Plugin hosting | Pro Tools AAX, Nuendo VST3 | VST3 / CLAP / AU | **PLANNED** | R14 | BE host | — | **VST3 SDK MIT** (as recorded in status); CLAP MIT; AAX proprietary | — | L | P2 | load reference plugin |
| Loudness & true peak | Nugen, iZotope | BS.1770-4/5, EBU R128, ATSC A/85 | **VERIFIED** (engine) | 0.000 LU vs ebur128; TP adjudicated | FE meters | — | — | — | S | P1 | reference signals |
| Surround & immersive | Pro Tools, Nuendo | 5.1/7.1/7.1.4, ADM BW64, Atmos | **PARTIAL** | VBAP panning to 7.1.4 | BE ADM · renderer | ADM | **EBU ADM renderer / libear Apache-2.0**; Dolby Atmos renderer proprietary; ADM profiles differ | — | L | P2 | libear parity |
| Dialogue editing, ADR, Foley | Pro Tools | Cue sheets, loop record, sync | **MISSING** | — | BE recording · FE | adr cues | — | — | L | P2 | — |
| Stems & deliverables | Pro Tools | Dialogue/music/effects stems, M&E | **PARTIAL** | stem kinds + render | BE · FE | audio.stem | — | — | M | P1 | — |

### 3.6 3D, CGI and animation

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 3D viewport | Blender, Maya | Interactive shaded viewport | **MISSING** | — | G raster · FE | scene prims | — | 60 fps | L | P2 | — |
| Scene description | OpenUSD | Composition, layers, variants, references | **PARTIAL** | USDA-shaped export (R17) | BE | previs.prim | **OpenUSD licence needs review** (R17) | — | XL | P2 | real `usdcat` reads output |
| Modelling | Blender, Maya | Mesh editing | **DEFERRED** | — | bridge to Blender | — | Blender GPL: separate process only (R01) | — | XL | P3 | — |
| Sculpting / texturing | ZBrush, Substance | — | **DEFERRED** | — | bridge | — | proprietary | — | XL | P3 | — |
| Rigging & animation | Maya, Blender | Rigs, keyframes, curves, constraints | **DEFERRED** | — | bridge first, native later | — | — | — | XL | P3 | — |
| Simulation (cloth, hair, fluids, destruction) | Houdini | — | **DEFERRED** | — | bridge | — | Houdini proprietary | — | XL | P3 | — |
| Rendering | Cycles, Arnold, RenderMan | Path tracing, AOVs to comp | **DEFERRED** | — | engine (TYPE B) | — | **Cycles Apache-2.0** (R01) | — | XL | P3 | — |
| Asset library & versioning | ShotGrid/Flow, Blender assets | Versioned assets used across shots | **PARTIAL** | asset kinds; content identity | BE versions · FE browser | asset.* | — | — | L | P2 | — |
| Mocap retargeting | MotionBuilder | — | **DEFERRED** | — | bridge | — | FBX SDK proprietary | — | XL | P3 | — |

The 3D stance is deliberate and comes from research 01: CINE-OMNI should be the
*project and pipeline* that 3D work plugs into — cameras, shots, assets, versions,
renders arriving as plates — long before it is a modelling or animation package.
Building a Blender replacement is years of a large team; bridging to Blender as a
separate process costs weeks and is legally clean.

---

## 4. DELIVERY

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Render / encode pipeline | Resolve Deliver, Media Encoder | Queue, background, exact frames | **PARTIAL** | `render.sequence` job (PNG 8/16-bit, QuickTime movie, or 32-bit float EXR): the cut as the monitor resolves it (retimes, active angles, gaps black) to PNG frames (8 or 16-bit) or a QuickTime movie (H.264 constrained baseline + 24-bit LPCM stereo 48 kHz from the player's own mixer, sample-exact per picture); as decoded or through the project view by the window's own bake; explicit output size and fit (refuse/letterbox/crop/stretch, Lanczos-3); captions burned in; cancellable, partial output removed; manifest with per-dependency fingerprints and a `current/stale/invalid/failed` state recomputed on request; Jobs tab form and render list with preview, download, reasons | ProRes/DNxHR/AAC · 16-bit movie · half-float EXR · deep/layered EXR · HDR mastering metadata · timecode track · GPU render · render presets | render job, `derived/render/*/manifest.json`, `renders` RPC, `/render-file`, `/render-frame` | H.264 via the build's existing `rusty_h264` (patent position unchanged); no new dependency | 46 frames 320×180 with sound 1,171 ms (dev); submit→visible 1.1–3.3 s in the browser (two runs) | L | **P1** | `render_movie.rs` (bit-exact sound, 0-sample retime sync, staleness), `mov_roundtrip.rs` (GStreamer `qtdemux` as oracle), `ui/e2e/render.mjs` (real app, headless Chrome) |
| Mezzanine masters (ProRes, DNxHR) | Resolve | Studio masters | **BLOCKED** | — | M encode | — | Apple ProRes licensing; FFmpeg LGPL route | — | M | P1 | — |
| Web / social deliverables | Resolve, Premiere | H.264/HEVC presets | **BLOCKED** | — | M encode | — | patent pools | — | M | P1 | — |
| DCP (theatrical) | DCP-o-matic, Clipster | SMPTE DCP: JPEG 2000 in MXF, CPL/PKL, ≤250 Mb/s | **MISSING** | — | M J2K encode · MXF | delivery package | **OpenJPEG BSD-2** (verified); SMPTE ST 429 | — | L | P2 | DCP validators |
| IMF (streaming masters) | Clipster, Resolve | App 2E: J2K + PCM, CPL, Photon-clean | **MISSING** | — | M · MXF | delivery package | **Netflix Photon is open source** (verify licence); OpenJPEG/OpenJPH BSD-2 | — | L | P2 | passes Photon |
| Timed text | Subtitle Edit, EZTitles | TTML1 / IMSC1.1 (Netflix), SCC, SRT | **VERIFIED** (IMSC, SRT, WebVTT, SCC) | `cine_timeline::captions`: IMSC 1.1 Text written with frame-exact offsets (`ttp:frameRate` + multiplier, refused for a rate TTML cannot express), SRT read/write and WebVTT write snapped to the sequence's frames, and delivery checks (lines, characters per line, duration, gap, reading speed, overlap). Checked against **ttconv**, an independent TTML reader: at 24000/1001, 30000/1001 and 25, including a half-millisecond tie and an hour in, every caption it decodes lands on the frame written. Reachable from the Captions tab (row above). SCC (CEA-608 pop-on) is written and read, with EOC-aligned line timing, erases spliced into a following load, 32-column rows refused past their width, special and extended characters, and re-quantisation onto 29.97 drop-frame reported; ttconv decodes it onto the frames written and found the one timing bug. No IMSC schema validation, no roll-up or paint-on | schema validation; roll-up | caption track | W3C specs; ttconv as oracle | — | M | P1 | `captions_oracle.rs` + 7 unit tests |
| Audio masters & stems | Pro Tools | BWF, ADM BW64, loudness-compliant | **PARTIAL** | stems + loudness | BE writers | audio.stem | — | — | M | P1 | loudness spec per platform |
| HDR deliverables | Resolve Studio | PQ/HLG with mastering metadata | **MISSING** | — | BE · M | mastering meta | Dolby for DV | — | L | P2 | — |
| Automated QC | Vidchecker, Baton | Black frames, freezes, loudness, gamut, flashes, spec | **MISSING** | loudness only | BE analysers | QC report | — | faster than real time | L | P2 | known-defect corpus |
| Checksums & archive packages | ASC MHL, LTO | Manifested deliverables, restorable projects | **PARTIAL** | content hashes; op log | BE MHL · archive | archive | — | — | M | P2 | restore project on clean machine |
| Project restoration | Every studio | A project reopens years later | **PARTIAL** | snapshot + op log; schema versioning absent | BE schema migration | project manifest | — | — | M | P1 | open old fixture after schema change |

---

## 5. FOUNDATION / PLATFORM

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Exact time model | OTIO, AAF | Rational time everywhere | **VERIFIED** | — | — | RationalTime | — | — | — | P0 | 25 adversarial tests |
| Project graph with propagation | (CINE-OMNI's own) | Changes flow to dependents; derived goes stale, captured survives | **VERIFIED** | 8 reactors (captions added); impact panel built from committed deltas, grouped by department, in every client | — | UCG | — | cascade <100 ms | M | **P0** | script edit flags only the bound clip (protocol + e2e) |
| Undo / redo / history | All | Grouped undo across departments | **VERIFIED** | shared history panel; a failed cascade stays undoable | — | op log | — | — | S | P0 | undo propagates (tested) |
| Persistence & crash recovery | All | No committed work lost | **VERIFIED** | SIGKILL tests | fsync durability not claimed | — | — | — | S | P0 | power-loss test (not claimed) |
| One runtime, many clients | Resolve collaboration | Shared live state | **VERIFIED** | loopback only | auth for remote | — | — | — | — | P0 | — |
| `cine start` single entry point | — | One command, whole app | **PARTIAL** | serves browser app; not native workspace | BE supervise native shell | — | — | open <2 s | M | **P0** | e2e test (browser part verified) |
| Capability reporting | (CINE-OMNI's own) | Honest states with reasons | **VERIFIED** | 28 capabilities | — | — | — | — | — | P0 | e2e test |
| Background job system | Resolve, Silverstack | Offload, proxies, renders, analysis — queued, resumable, visible | **VERIFIED** | `cine-jobs`: per-resource workers, priority, dependencies, cancel, retry, panic containment, dedup by key, journal with resume; runtime runners for checksum and thumbnails; live jobs panel | proxy / render / transcode kinds | job | — | bounded concurrency | L | **P0** (unblocks 10+ rows) | `jobs.rs` (8 guarantees), `jobs_protocol.rs` |
| Users, roles, permissions | Avid, ShotGrid | Departments see and edit their parts | **MISSING** | — | BE auth · FE | permission model | — | — | L | P2 | — |
| Remote collaboration & review | Frame.io, ShotGrid | Review links, timecoded comments | **MISSING** | — | BE · auth · M streaming | note on timecode | — | — | L | P2 | — |
| Plugin / extension API | OFX, VST3, Python in Resolve | Scripting and third-party tools | **MISSING** | — | BE API · sandbox | — | — | — | L | P2 | — |
| Storage policy & cache budgeting | — | Bounded caches, classified data | **PARTIAL** | policy written; no disk caches yet | BE cache manager | storage classes | — | — | M | P1 | budget enforcement test |
| Schema versioning / migration | All | Old projects open in new builds | **MISSING** | — | BE migrations | schema version | — | — | M | P1 | fixture from previous schema |
| Native + web shell convergence | Resolve (native), Frame.io (web) | One application experience | **PARTIAL** | two shells | decision + work (roadmap Phase 1) | — | — | — | L | **P0** | — |
| Accessibility & localisation | — | Keyboard, screen readers, languages | **MISSING** | keyboard partial | FE | — | — | — | M | P2 | — |
| Cross-platform builds | All | Linux, macOS, Windows | **PARTIAL** | Linux only verified | CI matrix | — | — | — | M | P1 | CI on three OSes |

---

## Summary counts

Counted from the tables above (154 rows; where a row has two states, the first is counted):

| Status | Count |
|---|---:|
| VERIFIED | 21 |
| IMPLEMENTED | 8 |
| PARTIAL | 50 |
| PROTOTYPE | 3 |
| PLANNED | 4 |
| MISSING | 51 |
| BLOCKED | 9 |
| DEFERRED | 8 |

The honest reading: **the foundation is verified, the product is mostly missing,
and the largest single category of "partial" is engine capability with no
interface** — which is also the cheapest category to turn into visible product.
