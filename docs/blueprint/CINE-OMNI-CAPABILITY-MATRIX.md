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
| Revisions | Final Draft revision mode | Colour revision sets, locked pages, A-pages, omitted scenes, collated revision reports — the way a production office expects | **MISSING** | op log exists; no revision model | BE revision sets · FE revision marks | script.revision | none | — | M | P1 | WGA revision colour sequence; locked page stability |
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
| Script breakdown & tagging | Movie Magic Scheduling, StudioBinder | One breakdown sheet per scene; tagged cast, props, wardrobe, vehicles, SFX, VFX, animals, extras | **PARTIAL** | breakdown derived from script | FE tagging UI · BE custom categories | logistics.breakdown | — | — | M | P1 | sheet parity with MMS categories |
| Stripboard | Movie Magic Scheduling | Drag strips to reorder shooting order, day breaks, colour by INT/EXT/DAY/NIGHT | **PARTIAL** | Production workspace: auto-scheduled board in industry colours, pages in eighths, constraints editable and rescheduling live; a script rewrite reaches the board (measured 14.2 ms cascade); **strips cannot be dragged** | BE manual strip ops · FE drag | logistics.schedule/day | — | 300 strips drag at 60 fps | M | P1 | `production_protocol.rs`, `ui/e2e/production.mjs` |
| Day out of days | Movie Magic Scheduling | Per-cast work/hold/travel/drop-pickup days | **MISSING** | — | BE report | logistics.day + cast | — | — | S | P1 | DOOD vs hand-computed |
| Call sheets | StudioBinder, MMS | Daily call sheet from schedule, crew, locations, weather, hospitals | **PARTIAL** | derived per day and shown in the Production workspace; regenerated on reschedule and on script change (tested); no weather, hospitals or PDF | PDF · real crew data | logistics.call_sheet | — | — | M | P1 | regenerate on reschedule |
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
| Card offload with verification | Silverstack, ShotPut Pro, Hedge | Copy to ≥2 destinations, verify, never mark verified what was not | **PLANNED** | state machine designed (R15) | BE jobs · FE queue | offload.job | — | I/O bound with xxHash (6.7 GiB/s CPU) | M | P1 | bit-flip detection; resume after pull |
| Checksum manifests | ASC MHL | ASC MHL v2 manifests, xxh64/xxh128, chain of custody | **PARTIAL** | whole-file SHA-256 established then verified as a job, mismatch recorded without overwriting the reference (catches a mid-file byte the fingerprint cannot); no MHL manifest, no xxHash | BE MHL writer/reader | offload.manifest | ASC MHL is BSD-style (verify) | ~40 MiB/s measured in a debug test build | S | P1 | validates in reference `ascmhl` |
| Media registration & identity | Every NLE | Content identity, relink after move, offline as a state | **VERIFIED** | content hash, relink, online flag | — | asset.media | — | — | — | P0 | relink test; visible in browser |
| Camera & lens metadata | Silverstack, ARRI/RED tools | Camera, lens, focal length, T-stop, focus distance, per frame where available | **PARTIAL** | kinds modelled (R15) | BE parsers · M | media metadata | RAW SDK terms (below) | — | L | P1 | round-trip vs camera reports |
| Takes, slates, circle takes | Script supervisor, Moviola | Take status vocabulary (not boolean), slate identity, circled takes | **PARTIAL** | takes bind to media | FE · BE status vocab | logistics.take | — | — | M | P1 | take survives shot regeneration (tested) |
| Multi-device take reconciliation | Silverstack, Resolve | Camera A/B + sound recorder merged into one take by TC/slate | **PLANNED** | match-key cascade (R15) | BE | take ↔ media[] | — | — | M | P1 | reconcile fixture of 5 streams |
| Sound reports & poly WAV | Sound Devices, Zaxcom | iXML/BEXT metadata, track names, scene/take | **MISSING** | WAV decode only | BE parser · M | audio metadata | — | — | S | P1 | iXML fixtures |
| Timecode & sync | Tentacle, Ambient | Jam-synced TC, audio sync by TC or waveform | **PARTIAL** | waveform sync (`cine-timeline::sync`), TC parse | FE · BE | sync.alignment | — | — | M | P1 | sync fixture ±1 frame |
| Dailies | Resolve, Colorfront, Pix | Graded, synced, watermarked review media, per-day | **MISSING** | — | BE render · M encode · G | dailies.* | encoder licensing | faster than real time | L | P1 | — |
| Proxy generation | Resolve, Premiere | Background proxies tied to originals, never promoted to source | **PARTIAL** | derivative model + provenance | BE encode · jobs | asset derivative | encoder licensing | background, bounded | M | P1 | proxy ≠ original asserted |
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
| Three-point / four-point editing | Avid, Premiere | In/out on source and record, fit-to-fill | **PARTIAL** | source in/out + record playhead; no record in/out, no fit-to-fill, no backtiming | FE record marks · BE | marks are editor state | — | — | M | **P0** | Avid semantics table (R16) |
| Ripple / roll / slip / slide / trim | All NLEs | Handle-bounded, reports applied delta, mixed rate | **VERIFIED** | drag tools with frame readout; runtime clamps and reports requested vs applied; one undo step | — | nle.* | — | edit → other client refetched 5.3 ms median | M | **P0** | protocol tests incl. clamping; UI gesture tests; e2e |
| Split / join / lift / extract / replace | All NLEs | — | **VERIFIED** (split, lift, extract) / **IMPLEMENTED** (join, replace) | razor / ⌘K, ⌫, ⇧⌫ in UI; join and replace reachable by protocol only | FE join/replace gestures | nle.* | — | — | S | **P0** | `linked_edits.rs`, e2e |
| Snapping | All NLEs | Cuts, markers, playhead; zoom-independent | **PARTIAL** | UI snaps drags to runtime-composed cut points and the playhead (8 px); markers not targets; engine `snap` unused | FE marker targets | — | — | — | S | P0 | `edit.test.ts` |
| Linked A/V | All NLEs | Linked selection and edits, sync-break indicators | **PARTIAL** | edits follow link groups in one transaction; the group agrees on one clamped delta; locked linked track refuses; **no sync-break indicator**, no linked selection | FE indicator | OTIO metadata | — | — | M | P0 | `linked_edits.rs`, protocol |
| Multi-selection & track targeting | All NLEs | Range and multi-clip selection | **MISSING** | — | FE · BE multi-op tx | selection (session, not project) | — | — | M | P0 | — |
| Markers | All NLEs | Clip and timeline markers with colour/notes | **PARTIAL** | modelled + projected; not editable | RPC · FE | nle.marker | — | — | S | P1 | — |
| Transitions | All NLEs | Dissolves, wipes, handle check | **PARTIAL** | modelled; **no handle check** | BE check · render | nle.transition | — | — | M | P1 | unrenderable dissolve refused |
| Multicam | Resolve, Premiere, Avid | Sync by TC/audio, angle switching live | **PLANNED** | R16 §A.7 | BE · FE · M multi-decode | multicam clip | — | 4 angles live | L | P1 | switch cut accuracy |
| Nested / compound sequences | All NLEs | Sequences as clips | **MISSING** | OTIO stacks exist | BE · FE | nle.stack | — | — | M | P1 | — |
| Keyframes & effects | All NLEs | Opacity, position, speed, audio levels; curves | **MISSING** | — | BE · FE · G | animated params | — | — | L | P1 | — |
| Retiming / speed ramps | All NLEs | Retime as a function, not a scalar | **PLANNED** | R16 §A.6 | BE · M · G | retime curve | — | — | M | P2 | — |
| Captions & subtitles | Resolve, Premiere, subtitle tools | SRT/TTML/IMSC/SCC authoring, burn-in | **MISSING** | — | BE · FE | caption track | — | — | M | P1 | IMSC validator |
| Proxy editing & conform | All NLEs | Cut on proxies, conform to originals exactly | **PARTIAL** | multiple refs preserved; proxy never becomes original | BE proxy gen · FE toggle | media refs | — | — | M | P1 | proxy/original round-trip (tested) |
| Relink / offline media | All NLEs | Relink by content, name, TC | **VERIFIED** (engine) / **IMPLEMENTED** (UI) | content-hash relink | FE relink dialog | asset.media | — | — | S | P1 | relink tests |
| OTIO interchange | OpenTimelineIO | Read and write real OTIO | **PARTIAL** | write Tier 1; read fails real files | BE reader hardening | nle.* | Apache-2.0 spec; no usable Rust binding | — | M | P1 | 5-tier harness (R16) |
| EDL | CMX3600 | EDL import/export | **MISSING** | — | BE | nle.* | — | — | S | P2 | round-trip vs Resolve |
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
| EXR / image sequences | OpenImageIO, OpenEXR | Multipart, deep, data windows, AOVs | **MISSING** | — | M · data windows | image sequence | OpenEXR BSD-3, OIIO Apache-2.0 | tile/scanline streaming | M | **P0** (blocks VFX) | OpenEXR test images |
| Audio decode | FFmpeg, symphonia | WAV/BWF, AIFF, AAC, MP3, FLAC | **PARTIAL** | symphonia (MPL-2.0) | M | — | MPL-2.0 file-level: fine unmodified | — | S | P0 | — |
| Colour metadata parsing | FFmpeg | `colr`, VUI, mastering display | **PARTIAL** | H.264 SPS/VUI parsed (clean-room from H.264 §7.3.2.1.1, Annex E); pictures carry what the stream declares, unspecified stays absent; `colr` box and mastering-display SEI not parsed | `colr` · SEI | media colour meta | — | — | S | **P0** | parsed size = decoded size on real streams (`sps_real.rs`) |
| Hardware decode | NVDEC, VA-API, VideoToolbox | Where available, with software fallback | **MISSING** | none claimed | M · G · OS APIs | capability registry | vendor APIs | — | L | P1 | parity vs software decode |
| Playback scheduler & decode-ahead | Every NLE | Real time, dropped-frame honesty | **VERIFIED** (native) | 144/144 at 24 fps | — | — | — | — | — | P0 | 60 tests + measured |
| Multiresolution / proxies in playback | Resolve | Choose decode resolution adaptively | **MISSING** | — | M · G | — | — | — | M | P1 | — |
| Frame cache & render cache hierarchy | All | RAM → disk → proxy, evicted by budget | **PARTIAL** | RAM frame cache, decode-ahead by bytes | BE disk cache | cache | storage policy | bounded | M | P1 | eviction under budget |

### 3.3 VFX and compositing

| Capability | Reference | What professionals need | Status | Now | Requires | Model | Deps / licensing | Perf | Cx | P | Validation |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Node graph | Nuke, Fusion | Typed nodes, caching, pull evaluation | **PROTOTYPE** | 28 node kinds, persistence | FE node editor · data windows | comp.graph/node | — | interactive on 2K | L | P1 | — |
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
| Display transforms | Resolve, Baselight | Correct, declared, never substituted | **PARTIAL** | ACES 2.0 exact on neutrals; saturated off ≤0.30; ACES 1.0 refused | BE · G | color.config | ACES Apache-2.0 | real time | M | P1 | OCIO oracle conformance (1,441 samples) |
| OCIO config loading | OpenColorIO | Load studio and ACES configs | **PARTIAL** | cannot load shipped ACES configs (re-verified) | BE parser or sanctioned OCIO | color.config | OCIO BSD-3-Clause (verified) | — | M | **P0** (for colour) | loads all 3 ACES configs |
| Primaries: wheels, lift/gamma/gain, offset | Resolve | Precise, GPU, per-shot | **MISSING** | CDL maths exists | FE wheels · G | color.grade | — | 4K real time | M | P1 | ASC CDL conformance |
| Curves | Resolve | RGB, hue-vs-X | **MISSING** | — | FE · G | grade | — | — | M | P1 | — |
| Qualifiers & windows | Resolve | HSL keys, power windows, tracking | **MISSING** | keyer exists | FE · G · tracking | grade nodes | — | — | L | P1 | — |
| Node-based grading | Resolve | Serial/parallel/layer nodes | **MISSING** | comp graph could host | FE · G | grade graph | — | — | L | P2 | — |
| LUTs & CDL | Resolve | 1D/3D LUT, .cube, CDL export | **IMPLEMENTED** | LUT, CDL | FE | color.lut/cdl | — | — | S | P1 | .cube round-trip |
| Scopes (waveform, parade, vectorscope, histogram) | Resolve | Accurate, real time, HDR-aware | **IMPLEMENTED** | `cine-color` scopes, not wired | FE · G | — | — | real time | M | **P0** (for colour) | known-signal scope tests |
| HDR (PQ/HLG) grading & monitoring | Resolve Studio | HDR scopes, mastering metadata | **MISSING** | — | G HDR surface · BE | mastering meta | — | — | L | P2 | — |
| Dolby Vision | Resolve Studio | Analysis metadata; trims | **BLOCKED** | — | Dolby tools | — | Dolby: basic metadata free in tools; trims licence $2,500/yr; SDK terms proprietary | — | L | P3 | — |
| Shot matching, stills/gallery | Resolve | Reference stills, split-screen compare | **MISSING** | — | FE · M | stills | — | — | M | P2 | — |
| Camera RAW / log colour science | Resolve | Log curves, gamut per camera | **PARTIAL** | some transforms | BE · RAW SDKs | — | vendor specs | — | M | P2 | vendor LUT parity |

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
| Render / encode pipeline | Resolve Deliver, Media Encoder | Queue, background, exact frames | **MISSING** | — | BE jobs · M encode · G | render job | encoder patents | faster than real time | L | **P1** | frame-exact output vs timeline |
| Mezzanine masters (ProRes, DNxHR) | Resolve | Studio masters | **BLOCKED** | — | M encode | — | Apple ProRes licensing; FFmpeg LGPL route | — | M | P1 | — |
| Web / social deliverables | Resolve, Premiere | H.264/HEVC presets | **BLOCKED** | — | M encode | — | patent pools | — | M | P1 | — |
| DCP (theatrical) | DCP-o-matic, Clipster | SMPTE DCP: JPEG 2000 in MXF, CPL/PKL, ≤250 Mb/s | **MISSING** | — | M J2K encode · MXF | delivery package | **OpenJPEG BSD-2** (verified); SMPTE ST 429 | — | L | P2 | DCP validators |
| IMF (streaming masters) | Clipster, Resolve | App 2E: J2K + PCM, CPL, Photon-clean | **MISSING** | — | M · MXF | delivery package | **Netflix Photon is open source** (verify licence); OpenJPEG/OpenJPH BSD-2 | — | L | P2 | passes Photon |
| Timed text | Subtitle Edit, EZTitles | TTML1 / IMSC1.1 (Netflix), SCC, SRT | **MISSING** | — | BE writers · validator | caption track | W3C specs | — | M | P1 | IMSC validator |
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
| Project graph with propagation | (CINE-OMNI's own) | Changes flow to dependents; derived goes stale, captured survives | **VERIFIED** | 7 reactors; impact panel built from committed deltas, grouped by department, in every client | — | UCG | — | cascade <100 ms | M | **P0** | script edit flags only the bound clip (protocol + e2e) |
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

Counted from the tables above (153 rows; where a row has two states, the first is counted):

| Status | Count |
|---|---:|
| VERIFIED | 17 |
| IMPLEMENTED | 5 |
| PARTIAL | 47 |
| PROTOTYPE | 3 |
| PLANNED | 6 |
| MISSING | 58 |
| BLOCKED | 9 |
| DEFERRED | 8 |

The honest reading: **the foundation is verified, the product is mostly missing,
and the largest single category of "partial" is engine capability with no
interface** — which is also the cheapest category to turn into visible product.
