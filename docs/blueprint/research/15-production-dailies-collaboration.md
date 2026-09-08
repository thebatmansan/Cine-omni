# CINE-OMNI Industry Blueprint — 15: Production, Dailies and Collaboration

**Status:** research, complete. Every interchange format named here was fetched
and read: ASC MHL (spec, XSDs, reference implementation, generated examples),
MHL v1.1, SMPTE ST 2114 (C4), iXML, EBU Tech 3285 (BWF), ALE, the ASC CDL schema,
ACES AMF, Cooke /i, SMPTE ST 12-1 / ST 380 / RDD 18. Vendor collaboration
behaviour is quoted from vendor documentation; the CRDT and OT literature from
the primary papers. Every Rust crate named has been verified on crates.io and
against its repository's LICENSE file. What remains unverified is listed in §G —
the largest single gap is Adobe Productions, whose documentation refused every
automated fetch.
**Date of research:** 2026-09-08
**Scope:** (A) on-set / script supervision / camera + sound metadata,
(B) dailies / DIT offload, checksums, manifests and verification,
(C) multi-user collaboration against the existing UCG op log.
**Companion documents:**
[`09-editorial-media-model.md`](09-editorial-media-model.md) (media identity,
offline states, relink — **not repeated here**),
[`../PHASE-2-MEDIA-ENGINE.md`](../PHASE-2-MEDIA-ENGINE.md) (content-addressed
identity as implemented),
[`10-color-and-sync.md`](10-color-and-sync.md) (colour metadata, A/V sync).

> **Evidence discipline.** Claims are either (a) *measured on this machine* with
> the measurement reproduced, (b) *read from the source tree* with the file and
> line named, (c) *quoted from a primary source* with a URL, or (d) marked
> **`UNVERIFIED`** inline and listed in §G. Nothing is asserted from memory
> without a marker. No proprietary schema is reproduced; the concepts are
> extracted and restated.

---

## 0. Findings summary

Eight findings, in order of how much they change what gets built.

**F1 — A derived-data reactor can currently delete captured production data.
Reproduced by execution against the working tree (§B.6).**
`cine_logistics::graph::write_shot_list` deletes any `logistics.shot` whose
previs camera is no longer in the input list
(`crates/cine-logistics/src/graph.rs`, the `for stale in children_of_kind(...)`
loop). `Tx::delete` recurses into `Contains` children
(`crates/ucg-core/src/tx.rs`). `write_take_metadata` attaches takes to shots with
`tx.link(shot, id, EdgeKind::Contains)`. Therefore **deleting a previs camera
deletes the takes shot on it**, through a reactor, without the user asking.
Measured: three captured takes deleted by one regeneration, four `DeleteNode`
ops, no user involvement (§B.6). The rule this yields is general and belongs in
`MODULE-CONVENTIONS.md`: *captured data must never be a `Contains`-child of
derived data, and no reactor may delete a node that has captured descendants.*

**F2 — `ContentHash::verify()` does not verify. Reproduced by execution
against the working tree (§B.5).** In
`crates/cine-media/src/identity.rs`, `verify()` recomputes a `Fast` hash when the
stored hash is `Fast` — head + tail + length only. A file whose *middle* is
corrupt passes. The Phase 2 doc is right that `Fast` and `Full` are "different
kinds, not different qualities", but the API lets a `Fast` hash answer a
verification question. Measured: a 4 MiB file with one bit flipped at offset 2 MiB passes
`Fast::verify()` and fails `Full::verify()` (§B.5). Verification must be defined to require
a whole-file digest; the `Fast` path should be renamed `matches_fingerprint()`.

**F3 — Checksums: two jobs, two algorithms, and the split already exists in the
code.** Measured on this machine, quiet (§B.4): XXH64 at **6 716 MiB/s**, XXH3-64 at
**5 334 MiB/s**, BLAKE3 (pure-Rust build) at **1 679 MiB/s**, MD5 at
**357 MiB/s**, SHA-1 at **292 MiB/s**, SHA-256 at **123 MiB/s** — so
**XXH64 is 55× SHA-256, and BLAKE3 is 14× SHA-256 while still being
cryptographic**. Independently corroborated by the ASC MHL reference
implementation, whose `ascmhl_default_hashformat` is **`xxh128`** and whose
permitted vocabulary contains **no SHA-256 and no BLAKE3** (§B.7.6). A card
offload verified with
xxHash is I/O bound; the same offload verified with SHA-256 is CPU bound by a
factor of ~30. Transfer verification (accidental corruption, non-adversarial)
wants xxHash; content *identity* wants a cryptographic digest. Recommendation in
§D.3: XXH64/XXH3 for transfer and manifest interop, BLAKE3 as the identity
migration target, SHA-256 retained until then, MD5/SHA-1 read-only for foreign
manifests.

**F4 — The identity key is not namespaced by algorithm.**
`media_entity_id()` builds `"media:file:<hex>"`
(`crates/cine-media/src/ucg.rs`). Changing or adding a digest algorithm later
would silently mint a *different* entity for the same file, or worse, collide
two different algorithms' hex strings into one key. Namespacing it now
(`media:file:sha256:<hex>`) costs one line and is unaffordable to change once
projects exist. The ASC MHL specification makes the same distinction structurally
— a hash element is named by its algorithm (`<md5>`, `<xxh64>`, `<c4>`), never a
bare digest string (§B.7.2) — which is the interchange format telling us the same
thing.

**F5 — A take is not traceable to media today, and cannot be with the current
edges.** `logistics.take` holds a metadata summary and events and binds to
nothing. Binding a take to N media files with N `Binds` edges cannot express
*which* file is A-camera, which is the sound recorder, what the timecode offset
is, or whether that pairing is verified — because `ucg_core::Edge` is
`{from, to, kind}` with no attributes. An intermediary node is required. §A.4
proposes `production.record`.

**F6 — The op log already carries `actor`, `lamport`, `cause` and `depth`; what
it lacks is identity and causality.** `Delta` has `revision: u64` (a *site-local*
counter), `cause: Option<u64>` (a reference to a site-local revision) and no
delta id, no parent set, no site id, no content hash. Every one of those is
needed before two op logs can ever be reconciled, and the op-log file format is
the single hardest thing in the system to change later. §C.5 gives the exact
struct.

**F7 — Reactor idempotence is the collaboration foundation, and it is already a
project rule.** Because every derived write is idempotent by convention
(`CLAUDE.md`: "re-running with the same input produces zero ops"), derived state
is *recomputable*. That means **only `depth == 0` deltas need to be replicated**;
everything a reactor produced can be regenerated locally and must converge. This
shrinks the merge problem from "the whole graph" to "authored data only", which
is a much smaller and much better-partitioned surface. §C.6 makes it a test.

**F8 — The ASC MHL standard's `action` vocabulary is exactly the verification
state machine, and its chain is weaker than our op log.** The specification's
closed enumeration is `original | verified | failed` and §5.6.4 states *"Only
hashes labeled as either `original` or `verified` can be used for verification.
Hashes labeled as `failed` cannot be used."* — the same absorbing-`failed` rule
§B.2 arrived at independently. Three things fall out that change plans: the ASC
MHL hash vocabulary is `md5, sha1, c4, xxh64, xxh3, xxh128`, so **SHA-256 and
BLAKE3 cannot be written into a manifest at all** and the transfer digest and the
identity digest are *forced* to be different (F3); the ASC MHL chain has **no
hash link between generations and no signature**, so it is interchange rather
than a chain of custody; and the reference implementation carries a rule worth
copying verbatim — a newly-added digest algorithm may only be recorded as
`verified` in a pass where an *already-recorded* algorithm verified. Full detail
and sources in §B.7.

### 0.1 The five decision questions, answered in one line each

| Q | Answer | Detail |
|---|---|---|
| **1. Data model for Film → Sequence → Scene → Shot → Take → Media** | `project` → `script.document` → `script.section` (**reuse as sequence — do not invent a kind**) → `script.scene` ← `DerivedFrom` ← `logistics.shot_list` → `Contains` → `logistics.shot` → `Contains` → `logistics.take` → `Contains` → **`production.record`** (new; one per device per take) → **`Binds`** → `media.material`. Traceability is a graph walk with no duplicated pointers. Two id schemes must change (take number and previs camera must stop being identities). | §A.3, §A.4, §D.1, §D.2 |
| **2. What must be true before "verified"** | Ten premises, all recorded on the node: whole-file digest; source digest read from the source; **destination digest re-read from disk after fsync**; equal lengths; equal digests under the same algorithm id; timestamp + verifier version present; and no path exists during the copy that a scan could mistake for finished media (temp + fsync + fsync(dir) + rename). Plus: a newly-added digest algorithm may only be recorded as `verified` in a pass where an already-recorded algorithm verified. Enforced by an unconstructible `Verified` type, not a boolean. Twelve named tests. The ASC MHL standard's closed `original\|verified\|failed` vocabulary corroborates the shape. | §B.2, §B.3 |
| **3. Which checksum** | **Two jobs, two algorithms — and the interchange format proves they cannot be unified.** XXH128 for *transfer verification* (measured on a quiet machine: XXH64 6 716 MiB/s vs SHA-256 123 MiB/s — 55×; and the ASC MHL reference tool's own default is XXH128). A cryptographic digest for *content identity*: SHA-256 today, BLAKE3 as the migration target. MD5/SHA-1 read-only for foreign manifests. | §B.4, R7–R10 |
| **4. Collaboration vs the existing op log** | **Optimistic concurrency over a causally-parented op log**, with advisory per-subtree claims. Replicate only `depth == 0` deltas and *recompute* derived state — available because reactor idempotence is already a project rule. Transactions must carry: `DeltaId`, `parents` (causality **and** Merkle chain), `site`, `author`, `wall_clock_utc`, `base_revision`, `content_hash`; `cause` becomes a `DeltaId`. CRDT and OT rejected for the graph, with reasons. | §C.3–§C.6 |
| **5. Minimum honest first slice** | (a) add the `Delta` metadata fields now — the op-log format is the least changeable artefact in the system; (b) write the authored-replay convergence test today; (c) single-writer / many-reader over a shared project directory (zero merge semantics, real value); (d) advisory `collab.claim` nodes; (e) the audit trail, which then costs almost nothing. **Not** in the slice: merge, CRDTs, a server, presence, permission enforcement. | §C.8 |


---

## A. On-set and production

> **Verification status of this section: the formats are verified; some
> workflow folklore is not.** §A.8 carries the primary-source detail — iXML,
> BWF `bext` (EBU Tech 3285 v2.0), ALE, ASC CDL, ACES AMF, Cooke /i, ZEISS
> eXtended Data, SMPTE ST 12-1 / ST 380 / RDD 18 — all fetched and read. §§A.1–A.7
> state the model; where a workflow claim rests on practitioner sources rather
> than a citable authority it says so inline and appears in the §G register.

### A.1 What the set actually produces

A shooting day produces five streams of record that must all end up pointing at
the same take, and which are created by five different people who cannot see
each other's work in real time:

| Producer | Artefact | Identifying keys it carries |
|---|---|---|
| Camera | one clip per take per camera | camera letter, reel/card, clip name, start timecode, rate |
| Sound | one polyphonic file per take (plus wild tracks) | sound roll, sound take number, start timecode, scene/take metadata written into the file |
| Script supervisor | lined script, daily report, continuity notes | scene, slate, take, circled/print, NG reason, duration, description |
| 2nd AC | camera report | roll, scene, slate, take, lens, filter, stop, remarks |
| DIT / data wrangler | offload manifests, LUTs, proxies | card id, checksums, copy destinations |

**The single most consequential fact for the data model: the camera's take
number and the sound recorder's take number are not the same number, and neither
is authoritative.** Sound rolls its own take count; a camera that rolls without
sound (MOS) or a sound-only wild track breaks any assumption that one take has
exactly one of each. `UNVERIFIED` as a citation, but it is why every professional
system reconciles by **timecode + scene/take metadata + duration**, not by
counting.

### A.2 Take status is a small, closed vocabulary — and it is not a boolean

"Circled take" (the take the script supervisor circles on the report as the one
to print/use) is the best-known status, but it is one value in a set that a
model must represent distinctly because they mean different things downstream:

**The vocabulary is not ours — iXML publishes it, and it is a *set*, not an
enum.** iXML v2.0 deprecated the individual booleans `<NO_GOOD>`,
`<FALSE_START>` and `<WILD_TRACK>` in favour of a comma-delimited `<TAKE_TYPE>`
drawn from `DEFAULT, NO_GOOD, FALSE_START, WILD_TRACK, PICKUP, REHEARSAL,
ANNOUNCEMENT, SOUND_GUIDE` — so `<TAKE_TYPE>FALSE_START,PICKUP</TAKE_TYPE>` is
legal and meaningful. `<CIRCLED>` was **not** deprecated and stays a separate
boolean (§A.8.1).

| Value | Source | Meaning | Downstream consequence |
|---|---|---|---|
| `circled` | iXML `<CIRCLED>` (boolean, independent) | the take the **director** selected; relayed by the script supervisor | dailies selects, editorial priority |
| `NO_GOOD` | iXML `TAKE_TYPE` | technically unusable | archived, not cut |
| `FALSE_START` | iXML `TAKE_TYPE` | aborted before the action completed | not in selects; must still be **retained** |
| `PICKUP` | iXML `TAKE_TYPE` | a partial re-take within a scene | must reference the take it picks up |
| `WILD_TRACK` | iXML `TAKE_TYPE` | sound with no picture | no camera record exists for it |
| `REHEARSAL`, `ANNOUNCEMENT`, `SOUND_GUIDE` | iXML `TAKE_TYPE` | not performance takes | excluded from take numbering expectations |
| `DEFAULT` | iXML `TAKE_TYPE` | an ordinary take | — |

The sound report's published abbreviation key adds a parallel vocabulary for the
sound side — `GD, NG, GT (guide track), RT (room tone), AMB, WSFX (wild sound
effect), WD (wild dialogue)` (§A.6) — which overlaps but does not coincide. Both
must be representable.

Four modelling consequences:

1. **Status is a flag set, not an enum.** A take can be a false start *and* a
   pickup. Typing it as a single-valued enum cannot round-trip a real iXML file.
2. **`circled` is not `good`.** Two independent axes: *technical usability* and
   *editorial selection*, and iXML keeps them as separate fields for exactly this
   reason. The authority chain is documented: *"the Script Supervisor will tell
   you which takes are to be circled. These are the takes that the Director
   likes"* (Elkins, §A.8.8) — different people, different facts.
3. **`pickup` needs an edge, not a flag.** A pickup take references a prior take.
   That is `References` (it does not propagate dirtiness — a note changing on the
   original must not dirty the pickup).
4. **A false start must be retained.** Any dailies workflow that treats
   `false_start` as "delete" violates the retention rule that everything shot is
   archived. Status controls *presentation*, never *retention*.

`series` takes remain in the model as a working concept; the term could not be
verified against a citable production-management source (§A.8.8, §G).

### A.3 Slate, scene numbering, and why the identity cannot be a number

Scene numbering in a shooting script is deliberately *stable under revision*:
when a new scene is inserted between 24 and 25 it becomes 24A ("24 apple"), so
that scene numbers on call sheets, breakdowns and camera reports keep referring
to the same material across coloured revisions. Slates are `scene + setup letter`
(24A, 24B — the setup, which is a different use of the same letter convention)
and takes are numbered within the slate.

**Verified, and with a rule worth implementing.** Elkins, *The Camera
Assistant's Manual* 5th ed.: *"When shooting a portion of a scene or a pick-up of
action within a scene, a letter is usually added to the scene number... **Some
letters that are not usually used for slating scene numbers are I, O, Q, S, and
Z, which can resemble numbers when written hurriedly.**"* Five excluded letters,
with the reason — so a slate-designation generator must skip **I, O, Q, S, Z**.

The convention is also **not universal**. Elkins: *"In Britain it is common
practice to not use scene numbers on the slate... they are written as shot
numbers, and the first shot on the first day of filming is shot number 1."* The
iXML specification says the same thing in its own `<SCENE>` definition (§A.8.1).
A model that assumes `"24A"` parses into `(24, 'A')` is wrong for half the world,
which is a second, independent reason the designation must be an opaque label.

The spoken form ("scene 24 apple") is **`UNVERIFIED`** — no canonical call-out
alphabet was found, and the practitioner lists that exist conflict with Elkins's
excluded set.

The data-model consequence is unambiguous and matches the media-identity
argument in [`09`](09-editorial-media-model.md) §1.8: **the human-facing
designation (`"24A"`, `"take 3"`) is a *label*, not an identity.** Renumbering
happens; takes get renumbered when a slate was mis-called; scenes get letters
inserted. So:

* `EntityId::derived(shot, "take:3")` — which is what
  `cine_logistics::graph::take_id` does today — makes the take *number* the
  identity. Renumber take 3 to take 4 and you have destroyed a node and created
  another, orphaning its media binding and its metadata.
* The take id must be minted once and never recomputed from a mutable label:
  `EntityId::new()` for an authored take, or `EntityId::from_key` over an
  **immutable** capture key (e.g. `"take:tc:<reel>:<start-timecode>"`) when the
  take is created by ingesting a camera clip.

This is the same mistake, in the same shape, as path-as-media-identity.

### A.4 The missing level: a take is recorded by several devices

A take is *one performance*. It is recorded by A-camera, B-camera and the sound
recorder simultaneously, each producing its own file with its own reel, its own
clip name and its own timecode. Editorial later syncs them.

Binding `logistics.take --Binds--> asset.media` N times cannot say which is
which. `ucg_core::Edge` is `{from, to, kind}` with **no attributes**
([`09`](09-editorial-media-model.md) §8.1 states this constraint), so the
per-recording facts — device role, reel, timecode offset, sync method, sync
confidence — have nowhere to live.

Therefore one node per (take × device):

```
logistics.take ─Contains→ production.record ─Binds→ asset.media (media.material)
                                │
                                └─References→ media.source   (the card/reel)
```

This is the same indirection AAF gets from `MasterMob` and FCPXML gets from
`mc-angle`/`mc-source`: a level whose entire job is to hold the per-representation
facts that the identity above it must not carry.

### A.5 Camera, lens and sound metadata already partly modelled

`ucg_schema::MetadataFrame` already exists
(`crates/ucg-schema/src/components.rs`) with `timecode_frames`, `iso`,
`shutter_angle_deg`, `focal_length_mm`, `f_stop`, `focus_distance_m`,
`white_balance_k`, `tint`, and `cine_logistics::metadata` already ingests it at
240 Hz into a bounded ring and reduces it to a `TakeMetadataSummary` with change
events (zoom, focus pull, aperture, ISO, white balance) using hysteresis.

That is a real, working on-set metadata path and it should not be rebuilt. What
it is missing, and what the model above needs:

| Missing | Why it matters | Where it goes |
|---|---|---|
| Lens identity (make, model, serial) | lens distortion/shading data is per *physical lens*, not per focal length; VFX needs the serial | `production.lens`, referenced |
| Entrance pupil, hyperfocal, near/far focus, HFOV | the actual per-frame payload of Cooke /i (§A.8.5) — and none of it survives into MXF | `production.record` |
| Timecode as a *labelled* value | frames alone cannot express drop/non-drop or origin | `cine_media::time::Timecode` (exists, drop-frame implemented) |
| Sync method and confidence | "jammed" ≠ "regen" ≠ "clap" ≠ "waveform" and editorial must know which | `production.record` |
| Sound channel → role mapping | a 8-channel poly file's channel 3 is "boom", not "audio 3" — iXML models this properly (§A.8.1) | `production.record` |
| Metadata *provenance* | live-ingested vs read from the file vs typed by a human are different confidence levels | attr on each value's carrier |

### A.6 Reports are derived, and that is the whole point

The camera report, sound report, script supervisor's daily report and the daily
production report are all **views over the same take records**, produced by
different departments and reconciled at the end of the day. In a graph where
takes are first-class, each is a derived artefact:

```
logistics.take (authored, on set)
   └─ DerivedFrom ← production.camera_report   (per camera per day)
   └─ DerivedFrom ← production.sound_report    (per sound roll per day)
   └─ DerivedFrom ← production.daily_report    (per shoot day)
```

This is exactly the pattern `cine-logistics` already uses for breakdowns and call
sheets, and it inherits the property that matters: the reports cannot drift from
the takes, because they are recomputed.

**Real field sets, from published forms rather than invention.** Camera report
(Florida State University's production handbook, printed labels): header
`PROD. NUMBER · PRODUCTION TITLE · DATE · RDM/ROLL # · SHEET # · MEDIA ID # ·
CAMERA ID # · DIRECTOR · 1st AC · FILE FORMAT · PROJECT FRAME RATE ·
DIR. OF PHOTOGRAPHY · 2nd AC · FRAMING ASPECT RATIO · LUT`; columns
`SCENE # | TAKE # | CLIP NAME | % | LENS | F-STOP | FILTER/REMARKS`.
Sound report (Rabiger & Hurbis-Cherrier, Focal Press 2020): header
`Project Title · Location · Date · Dir. · Recordist · Boom Op. · Bits ·
Sample Freq. · Format · Recorder · Fr. Rate · SOUND ROLL · Head Tone`; columns
`PNO # | Duration/T.C. | SCENE | TAKE | COMMENTS | Ch.1 | Ch.2 | Ch.3 | Ch.4`.
Script supervisor continuity log (same source):
`scene | tk | sr# | cr# | action | notes | time`.

Two structural facts confirm §A.4's model rather than merely illustrating it:
the sound report is keyed by **sound roll** and the camera report by **camera
roll / media id**, and each has its own channel or lens columns. They are
per-device records of a shared take — which is precisely what
`production.record` is.

The sound report's own abbreviation key (same source) is also a real, published
status vocabulary and it distinguishes more than "good/NG":
*"GD = Good; NG = No Good; GT = Guide Track; RT = Room Tone; AMB = Ambience;
WSFX = Wild Sound Effect; WD = Wild Dialogue"*.

**But see F1.** Derived reports being `DerivedFrom` a take is safe. A take being
`Contains`-ed by a *derived* shot is not.

### A.7 Reconciling five streams into one take — the match-key cascade

Nobody types the take structure in. It has to be *inferred* from files that
arrive separately, and the inference must record how confident it was. This is
the same problem as editorial relink ([`09`](09-editorial-media-model.md) §3.2),
one stage earlier, and it deserves the same treatment: a cascade of
progressively weaker keys, with **the key that matched recorded on the result**.

| Rank | Key | Matches | Fails when |
|---|---|---|---|
| 1 | Embedded scene/take metadata written by the device | camera clip ↔ sound file ↔ take | the device wasn't configured; the AC didn't update the slate metadata |
| 2 | Overlapping timecode + same shoot day + same rate family | camera ↔ sound | timecode was never jammed, or drifted; free-run vs rec-run mismatch |
| 3 | Reel/card + clip name ordering within a roll | camera clips ↔ camera roll | a card was reformatted and numbering restarted |
| 4 | Filename convention (`A001C003_...`) | camera clips | conventions differ per camera vendor and per production |
| 5 | Audio waveform correlation against the camera's scratch track | camera ↔ sound | no scratch audio (a hard MOS take) |
| 6 | Human assignment | anything | — |

Three rules fall out, and all three are the *same* rule the media model already
enforces one level up:

1. **Record which key matched, on the `production.record`.** "Grouped by
   timecode" and "grouped because the filenames sorted next to each other" are
   different confidence levels, and the assistant editor who has to trust the
   sync needs to see which one they are trusting.
2. **A failed match is a state, not an error.** An unmatched camera clip is an
   *unassigned record*, visible and correctable — never a dropped file, and
   never silently attached to the nearest take. This mirrors "offline is a
   state, not a failure" ([`09`](09-editorial-media-model.md) §3.1).
3. **Reconciliation must be idempotent and must never destroy a human
   assignment.** Re-running ingest after more files arrive has to converge
   (`CLAUDE.md`'s zero-op rule), and a match a person made by hand outranks
   every automatic key — so the match rank is stored, and automatic
   reconciliation refuses to overwrite rank 6.

### A.8 The interchange formats, verified against their specifications

Everything in this section was fetched and read. Licence and openness are stated
first for each, because that decides whether we may implement it at all.

| Format | What it carries | Openness | Licence |
|---|---|---|---|
| **iXML** | production semantics in a WAV chunk: scene, take, sound roll, circled, take type, channel roles | open, revision 3.01 (28 Oct 2021), `ixml.info` | **the format is released to the public domain**; the *prose describing it* is © Gallery UK |
| **BWF `bext`** | time and identity: sample-accurate origin time, originator, UMID, loudness, coding history | open, EBU **Tech 3285 v2.0** (May 2011) | EBU publication |
| **ALE** | Avid's tab-separated log: tape, timecode, scene, take, rolls, CDL columns | no formal spec; header syntax published in Avid's own whitepaper | vendor document |
| **ASC CDL** | slope/offset/power/saturation | **spec is NOT publicly downloadable** — `theasc.com` 403s; obtained by e-mail request | none stated |
| **ASC CDL XSD** | the actual schema | **public**, bundled in the ACES AMF repo | Apache-2.0 (as part of that repo) |
| **ACES AMF** | the colour pipeline as a sidecar, referencing CDL | fully open, spec updated Nov 2025 | **schema Apache-2.0, prose CC BY 4.0** |
| **Cooke /i** | per-frame lens data | "open protocol", but **extended commands are by-request/NDA** | © Cooke Optics; manual no longer self-hosted |
| **ZEISS eXtended Data** | /i plus distortion and shading | tools published, **no format spec found** | — |
| **SMPTE ST 12-1, ST 380, RDD 18** | timecode, MXF descriptive metadata, lens metadata | **free since 17 June 2026** at `pub.smpte.org` | SMPTE |

**A note that changes the sourcing posture for the whole blueprint: SMPTE's
entire catalogue became free on 17 June 2026** and is machine-fetchable at
`pub.smpte.org/doc/<id>/<date>-pub/<id>-<date>-pub.zip`. ST 12-1, ST 380,
RDD 18 and EG 42 were downloaded without authentication for this document.
Normative text can now be cited rather than paraphrased.

#### A.8.1 iXML — where production semantics actually live

The `iXML` chunk (`0x69584D4C`) carries raw XML in a WAV file. The element names,
verbatim from the specification's object details:

* `<PROJECT>` — *"The name of the project... typically the name of the motion
  picture or program"*
* `<SCENE>` — *"The name of the scene / slate being recorded. **For US system this
  might typically be 32, 32A, 32B, A32B, 32AB etc. For UK system this might
  typically be a incrementing number with no letters.**"*
* `<TAKE>` — *"The number of the take... variations for things like wild tracks
  may yield takes like 1, 2, 3, WT1, WT2"*
* `<TAPE>` — *"**The SoundRoll** which identifies a group of recordings... the
  soundroll should change at least once in any 24 hour period."*
* `<CIRCLED>` — *"mark this recording as a circle-take. The value should be TRUE
  or FALSE. If absent, this should be assumed FALSE."*
* `<FILE_UID>` — *"A unique number which identifies this physical FILE,
  regardless of the number of channels"*
* `<UBITS>` — *"The userbits associated with this recording... **Typically the
  userbits are rarely used now** because other more explicit metadata supercedes
  this function."*
* `<NOTE>` — free text, e.g. *"TAIL SLATE, NO SLATE, or to warn of noise
  interruptions - PLANE OVERHEAD"*

Five things here are directly load-bearing for §A.2, §A.3 and §A.4, and each
either confirms or corrects this document's model:

1. **`<TAPE>` is the sound roll, not a videotape.** The most misread field in the
   format, and a parser that treats it as a video reel will mis-key every
   reconciliation in §A.7.
2. **`<TAKE>` is a *string*, not a number** — `WT1`, `WT2` are legal values. This
   confirms §A.3: the take designation is a label. A schema that types it as an
   integer cannot round-trip a real sound report.
3. **`<SCENE>` explicitly documents the US letter convention *and* the UK
   numeric one** in the spec text itself. Any model that assumes `24A` parses
   into `(24, 'A')` is wrong for half the world.
4. **The status vocabulary is a *list*, and it superseded the booleans.**
   `<NO_GOOD>`, `<FALSE_START>` and `<WILD_TRACK>` are each marked
   *"(Deprecated in iXML v2.0, superceded by TAKE_TYPE)"*. `<TAKE_TYPE>` is a
   comma-delimited list drawn from
   `DEFAULT, NO_GOOD, FALSE_START, WILD_TRACK, PICKUP, REHEARSAL, ANNOUNCEMENT,
   SOUND_GUIDE` — e.g. `<TAKE_TYPE>FALSE_START,PICKUP</TAKE_TYPE>`. **`<CIRCLED>`
   was *not* deprecated**, and remains a separate boolean.

   This is a **correction to §A.2**: take status is not one value from a closed
   set, it is a *set* of orthogonal type flags, plus an independent `circled`
   boolean. §A.2's claim that "circled is not good" is right and the format
   agrees — but the rest of the statuses compose rather than exclude. A take can
   be a false start *and* a pickup. Model it as a flag set, not an enum.
5. **Channel roles are a first-class structure**, exactly as §A.5 requires:

   ```xml
   <TRACK_LIST>
     <TRACK_COUNT>2</TRACK_COUNT>
     <TRACK>
       <CHANNEL_INDEX>1</CHANNEL_INDEX>
       <INTERLEAVE_INDEX>1</INTERLEAVE_INDEX>
       <NAME>Mid</NAME>
       <FUNCTION>M-MID_SIDE</FUNCTION>
     </TRACK>
   </TRACK_LIST>
   ```

   `CHANNEL_INDEX` *"communicates the 'input' or 'source' number on a recorder"*
   while `INTERLEAVE_INDEX` gives position in the file, because *"XML does not
   require the objects to be in any particular order"*. Both are **1-based**:
   *"Channel and Interleave Indexes are indexed from a base of 1 (not zero)."*

Two further rules should be adopted verbatim as implementation policy:

> *"This would include the **iSCENE=, iTAKE=, iTAPE=, iNOTE=** metadata currently
> written by most recorders into the bext description field... **Reading
> applications should favour iXML if it is present, falling back to bext
> description if not.**"*

> *"All iXML parameters and objects are OPTIONAL. Readers must not assume that
> any particular parameters exist... **It is better for iXML writers to exclude an
> object or parameter than to simply add it with blank metadata.**"*

That second rule is the same distinction CINE-OMNI already draws in
`ColorMetadata::is_complete()` — **absent is not empty** — and it pairs exactly
with the idempotent-writer requirement: writing a blank field is an op, and an op
that carries no information is a convergence hazard.

**There is deliberately no XSD.** Verbatim: *"At the present time, to ensure that
no products are developed which enforce schema compatibility, it has been decided
**NOT to create a schema for iXML**."* So the parser must ignore unknown elements
rather than reject them; vendor subtrees (`<AATON_CANTAR>`, `<AVID>`,
`<STEINBERG>`) are expected.

`<SPEED>` is worth one more note because it vindicates an existing project rule:
all rates are expressed as **integer/integer ratios** — `30000/1001`,
`24000/1001` — alongside `TIMECODE_RATE` and a `TIMECODE_FLAG` of NDF/DF. The
professional sound format independently reached the same conclusion as
`crates/cine-media`'s exact-rational time model, and for the same reason.

#### A.8.2 BWF `bext` — and an important negative result

EBU Tech 3285 v2.0 defines the chunk exactly:

```c
CHAR  Description[256];        /* ASCII : «Description of the sound sequence» */
CHAR  Originator[32];
CHAR  OriginatorReference[32];
CHAR  OriginationDate[10];     /* ASCII : «yyyy:mm:dd» */
CHAR  OriginationTime[8];      /* ASCII : «hh:mm:ss» */
DWORD TimeReferenceLow;        /* First sample count since midnight, low word */
DWORD TimeReferenceHigh;
WORD  Version;
BYTE  UMID_0 .... BYTE UMID_63 /* Binary byte 0..63 of SMPTE UMID */
WORD  LoudnessValue;           /* Integrated Loudness Value in LUFS (x100) */
WORD  LoudnessRange;
WORD  MaxTruePeakLevel;
WORD  MaxMomentaryLoudness;
WORD  MaxShortTermLoudness;
BYTE  Reserved[180];
CHAR  CodingHistory[];
```

Fixed part is **602 bytes**, then variable `CodingHistory`. Offsets:
`Description` 0, `Originator` 256, `OriginatorReference` 288, `OriginationDate`
320, `OriginationTime` 330, `TimeReferenceLow` 338, `TimeReferenceHigh` 342,
`Version` 346, `UMID` 348, `LoudnessValue` 412, `LoudnessRange` 414,
`MaxTruePeakLevel` 416, `MaxMomentaryLoudness` 418, `MaxShortTermLoudness` 420,
`Reserved` 422, `CodingHistory` 602.

The field that matters most is `TimeReference`, and its definition is a trap:
*"a 64-bit value which contains the first sample count since midnight. **The
number of samples per second depends on the sample frequency** which is defined
in the field `<nSamplesPerSec>` from the `<format chunk>`."* So start-of-file
timecode is **a sample count, not a timecode**, and converting it requires the
sample rate from a *different chunk*. Storing it as a timecode string on ingest
would discard the precision that makes it useful for sync.

Loudness fields are signed with a sentinel: **`7FFFh` means ignore**, and *"If
any parameters are found to have values outside their valid ranges (not just
7FFFh)... they shall be ignored too."*

**The negative result.** EBU Tech 3285 v2.0 **never mentions iXML** — zero
occurrences — and contains no concept of scene, take or slate. The only hook is
§2.1: *"Any additional chunks that are present in the file and which are not
specified in this document... are to be considered **private**... However,
**BWF-compliant applications shall pass these chunks.**"*

That single "shall pass" is the entire legal basis on which iXML survives a BWF
round trip. The conclusion for the model is clean and worth stating as a rule:

> **BWF gives you time and identity. iXML gives you production semantics.
> Neither substitutes for the other, both must be read, and iXML wins where they
> overlap.**

#### A.8.3 ALE — and a finding that contradicts the common assumption

Avid publishes no formal specification, but its own metadata whitepaper prints
the header syntax:

```
Heading<RETURN>
FIELD_DELIM<TAB>TABS<RETURN>
VIDEO_FORMAT<TAB>NTSC<RETURN>
FILM_FORMAT<TAB>35mm, 4 perf<RETURN>
AUDIO_FORMAT<TAB>48khz<RETURN>
FPS<TAB>24<RETURN>
```

Three sections — `Heading`, `Column`, `Data` — tab-separated, and *"the Column
headings can appear in any order as long as the column data appears below the
correct heading."* Minimum importable set: *"1. Tape 2. Start 3. End"*.

Avid's own column definitions, verbatim, and note how precisely they match §A.4's
per-device split: `Tape` — *"ID of source. When entered, it triggers the creation
of TAPE MOB ID within Avid editing system"*; `Soundroll` — *"ID of sound roll. A
Clip must contain an Audio track in order for Soundroll to have a value."*;
`Camroll` — *"ID of individual camera roll"*; `Labroll` — *"ID of multiple camera
rolls when combined into one roll for transfer"*; `End` — *"Timecode of last
frame in clip **+1 (EDL Style)**"*.

That `End` definition is an exclusive-end convention and must not be silently
converted to an inclusive one.

> **⚠️ `Circled` is not a standard ALE column.** A real 129-column Avid export
> (from the OpenTimelineIO ALE adapter's test corpus) was indexed
> programmatically: it contains `Scene`, `Take`, `Camroll`, `Soundroll`,
> `Labroll`, `Reel`, `Tape`, `TapeID`, `Start`, `End`, `ASC_SOP`, `ASC_SAT`,
> `LUT`, `Comments` — and **no column whose name contains "ircl"**. Neither does
> Avid's own whitepaper column table. Circled-take is a *custom* column by
> convention, which Avid supports (*"Any new column (header) written to the ALE
> file will be available as custom fields in the Avid Media Composer"*), not a
> standard field.

The practical consequence: **the single most important editorial fact on set —
which take the director liked — has no standardised carrier in the dominant
editorial interchange format.** It travels in iXML `<CIRCLED>` on the sound file,
in a custom ALE column, or on paper. Any system that wants it must read it from
the sound side and carry it itself. That is an argument for `circled` being a
first-class attribute on `logistics.take` in our graph rather than something we
expect to import.

CDL columns carry real values in the shape:

```
ASC_SOP: (1.0000 1.0000 1.0000)(0.0000 0.0000 0.0000)(1.0000 1.0000 1.0000)
ASC_SAT: 1.0000
```

#### A.8.4 ASC CDL and ACES AMF — the licence asymmetry

**The ASC CDL specification is not publicly available.** `theasc.com` returns
HTTP 403 to every path and there is no archived copy of the release document;
the documented route is an e-mail request to the ASC. **No open licence is
stated.**

**But the official XSD is public**, bundled into the ACES AMF repository
(Apache-2.0), `targetNamespace="urn:ASC:CDL:v1.01"`, authored 20 July 2007. It
settles the alias question that the ALE columns raise:

```xml
<element name="SOPNode" type="cdl:SOPNodeType" substitutionGroup="cdl:ColorNodeBase"/>
<element name="ASC_SOP" type="cdl:SOPNodeType" substitutionGroup="cdl:SOPNode"/>
<element name="SatNode" type="cdl:SatNodeType" substitutionGroup="cdl:ColorNodeBase"/>
<element name="ASC_SAT" type="cdl:SatNodeType" substitutionGroup="cdl:SatNode"/>
```

So **`ASC_SOP`/`ASC_SAT` are the specification's own aliases for
`SOPNode`/`SatNode`** — the ALE column names are not an Avid invention.
`ColorCorrection/@id` is `anyURI` and **required**. Real files: `.cdl` has root
`<ColorDecisionList xmlns="urn:ASC:CDL:v1.01">`, `.ccc` has
`<ColorCorrectionCollection>`, `.cc` is a bare `<ColorCorrection>`. OpenColorIO
(BSD-3) additionally accepts `SATNode` as an alternate spelling and parses
scientific notation and leading-dot floats — accept both.

The maths, from the ACES CLF specification (CC BY 4.0):

> out_SOP = CLAMP(in × slope + offset)^power
> luma = 0.2126 × out_SOP,R + 0.7152 × out_SOP,G + 0.0722 × out_SOP,B
> out = CLAMP[luma + saturation × (out_SOP − luma)]

with styles `Fwd`, `Rev`, `FwdNoClamp`, `RevNoClamp` — the no-clamp variants
existing *"to provide compatibility with the many applications that take that
alternative approach"*. **The luma weights are Rec.709**, which is a fact a
colour-managed pipeline must not silently re-derive from the working space.

`ucg_schema::Cdl` already stores exactly slope/offset/power/saturation, so the
existing component is correct and needs only the clamp-style flag.

**ACES AMF is the genuinely open one**: schema Apache-2.0, prose CC BY 4.0,
namespace `urn:ampas:aces:amf:v2.0`, and it **imports the ASC CDL schema
directly**. Its root is `acesMetadataFile` → `amfInfo` (uuid, author, dateTime),
`clipId?`, `pipeline`, `archivedPipeline*`. Every transform carries a **required
`@applied` boolean**, which is the interesting design idea: the same file is
either a *recipe* (not yet applied) or a *receipt* (already baked), and the flag
says which. AMF also states its own boundary — *"AMFs do not contain 'timeline'
metadata such as edit points... Edit Decision Lists (EDLs) or Avid Log Exchange
files (ALEs) may reference AMFs"* — which is exactly the `AMF_UUID`/`AMF_NAME`
ALE column mechanism.

#### A.8.5 Lens metadata — and where it is lost

Cooke /i carries, verbatim from Cooke: *"**focal length, focus distance, zoom
position, near and far focus, hyper focal distance, T-stop, horizontal field of
view and entrance pupil position**... digitally recorded for every frame, at any
frame rate up to 285 fps"*. It is described as *"an open protocol"*, but the
support page qualifies that: *"if you wish to... **need access to any of the
non-disclosed technical information**, please contact us"*, and the manual splits
**CORE** commands (*"supported by ALL /i Technology partners"*) from **EXTENDED**
(*"available by request to /i Technology Partners"*).

ZEISS eXtended Data is *"**Based on the Cooke /i Technology**"* and adds
*"Lens distortion characteristics based on the focal point"* and *"Lens shading
characteristics based on the focal point and effective T-stop"*. ZEISS is careful
that *"**Shading does not refer to vignetting**"*. Carriage is a sidecar:
*"Side car files with extension **.zlcf (ZEISS Lens Correction File)**"*, and
*"none of the EXR transcoders... will automatically inject ZEISS eXtended Data
into the EXR file sequences. This must be done manually."*

> **The finding that matters: lens metadata is lossy at the container boundary.**
> SMPTE **RDD 18:2021** is the standardised Lens Unit Metadata Set — iris
> F/T-number, iris/focus/zoom ring positions, focus distance from image plane and
> front vertex, focal length, anamorphic squeeze, extender, and a free-text
> `Lens Attributes` capped at *"less than 64 bytes"*. It has **no entrance pupil,
> no lens serial, no hyperfocal, no near/far focus, no HFOV, and no
> distortion or shading**. Everything /i carries beyond focus/iris/zoom either
> dies at the MXF boundary or goes into a vendor blob.
>
> Therefore: **model the /i field set as the source of truth and treat RDD 18 as
> a lossy projection**, not the other way round. A `production.lens` node with
> the full /i payload plus a `.zlcf` reference is the honest shape; deriving our
> model from the SMPTE standard would silently discard what VFX actually needs.

#### A.8.6 Timecode, user bits and jam sync — measured drift

**User bits**, ST 12-1 §8.4, verbatim: *"The binary groups are intended for
storage and transmission of data by the users... **Note: The binary groups are
commonly referred to as 'user bits'.**"* Thirty-two bits in eight 4-bit groups,
with the interpretation governed by BGF2/BGF1/BGF0 — `000` unspecified, `100`
date and time zone (per ST 309), `101` page/line (per ST 262), `110` clock time
plus date, `111` clock time plus page/line. §8.4.1: *"The 32 bits within the
eight binary groups may be assigned in any manner without restriction."*

In practice they carry the date. Sound Devices: *"Static, numeric data that is
available as part of a timecode signal. **User bits are often used to indicate the
date of a file.**"* — with UI options `mm:dd:yy:UU` **and** `dd:mm:yy:UU`.

> **Rule: store user bits as four raw bytes plus a BGF-derived interpretation
> hint. Never parse them as a date without the BGF flags saying so** — vendors
> disagree on day/month order and one vendor exposes both.

**Jam sync**, defined by ARRI, verbatim: *"The camera... **samples the timecode
value once** from the timecode connector and then continues counting based on its
own high precision crystal clock... **This mode ensures stable timecode with an
offset of less than one frame for each eight hours, after which the camera has to
be re-jammed.**"* Contrast **Regen**, where *"the timecode source must remain
connected"*.

Vendor drift figures, all from vendor documentation:

| Device | Stated accuracy | Oscillator |
|---|---|---|
| ST 12-1, unreferenced LTC source | *"the frequency tolerance shall be **±100 ppm**"* (≈2.6 s/day) | — |
| Sound Devices 833 | *"Sample/Timecode Accuracy: **0.1 ppm (0.25 frames per 24 hours)**"* | — |
| Ambient NanoLockit | *"drift of less than 1 frame a day (**~0,1ppm**)"*; *"Zero Drift with active C-Jam Mode"* | — |
| Ambient Lockit | *"**drift less than 1 frame apart within 24 hours**"* | TCXO |
| Tentacle SYNC E | *"**Less than 1 Frame Drift in 24 Hours**"*; *"High Precision TCXO — **0.2 ppm**"* | TCXO |
| Deity TC-1 | *"**±0.5ppm**... **less than 1 frame of drift in a 72-hour period**"* | TCXO |
| Atomos UltraSync ONE | *"Free Running: **TCXO 0.5 ppm (~1 Frame Drift in 24 Hours)**"* | TCXO |
| ARRI ALEXA 35, jammed | *"less than one frame for each eight hours"* | crystal |
| TASCAM CG-1800 (master clock, for contrast) | *"**±0.01ppm**... frequency stability **±0.005ppm per day**"* | OCXO |

Sanity check, derived: at 24 fps one frame is 41.67 ms, so over 86 400 s one
frame/day is **≈0.48 ppm**. The 0.5 ppm ⇒ ~1 frame/24 h claims are internally
consistent; Sound Devices' 0.1 ppm ⇒ 0.25 frames/24 h checks out; Deity's
0.5 ppm ⇒ <1 frame/72 h only holds for parts well inside spec, so treat it as
optimistic.

**The model consequence:** a jam is an *event with an age*, and sync confidence
decays from it. `production.record` should carry `jammed_at` and the generator's
rated ppm, so "these two devices were jammed eleven hours ago from a ±0.5 ppm
source" becomes a computable expected offset rather than a guess — which is
exactly what §A.7 rank 2 needs to decide whether a timecode match is trustworthy.

**Genlock is not timecode**, and the clearest statement is Ambient's own:
*"while timecode is a required information to jam sync several units together...,
**it will not take care to keep them synchronous over a longer time as it will not
affect the speed the camera is generating frames**. To achieve this a blank signal
commonly referenced to as **genlock** is required."*

**Run modes**, Sound Devices' glossary: *"**record run** - timecode advances only
when recording is engaged · **free run** - timecode run continuously... ·
**24 hour** - similar to free run except the start time corresponds to
time-of-day · **ext TC** - the recorder applies the value of an external timecode
source."* ARRI adds the constraint that decides the data model: *"**Free Run
timecode is only possible if the sensor frame rate matches the project rate.** If
the camera is over- or undercranking... the camera will automatically switch to
run mode Rec Run."* So the run mode is a **per-record** fact, not a per-project
one, and it changes when someone ramps.

#### A.8.7 MXF DMS-1 — why not to model slates on it

SMPTE **ST 380** (there is no "ST 380-1"; the document is SMPTE 380M-2004) is
free and was read. It defines three frameworks — Production, Clip and Scene.
The relevant fields exist:

| UL | Name | Type | Framework |
|---|---|---|---|
| `01.05.06.00.00.00.00.00` | Scene Number | ISO 7-bit char, ≤32 | **Scene** |
| `01.05.07.00.00.00.00.00` | Take Number | **Uint16** | **Clip** |
| `03.02.05.03.00.00.00.00` | Slate Information | UTF-16 string | **Clip** |

> **Recommendation: do not model slates on DMS-1.** Four reasons, all from the
> standard itself: (a) `Take Number` is a **Uint16**, so it cannot express "3A",
> "WT1" or any pickup/series convention — which iXML's string `<TAKE>` can;
> (b) `Scene Number` lives in the **Scene** framework while `Take Number` lives
> in the **Clip** framework, so a slate's (scene, take) pair **is not one
> record** and must be reconstructed by correlating DM segments on different
> tracks; (c) `Slate Information` is untyped free text; (d) there is no camera
> roll, camera id, sound roll or circled flag anywhere in it.

And it was never widely adopted. The EBU's own Technical Review (2010 Q3):
*"**Until now, the use of DM inside MXF containers has not been widespread.**
Some cameras record titles and similar information as DMS-1... Still, **most
applications resort to basic key/value pairs or store DM in ancillary XML
files.**"* Corroborated directly: ARRI's own MXF metadata specification cites
ST 377-1, ST 378 and RDD 18 and contains **zero** occurrences of "380".

Treat DMS-1 as an export target at most.

#### A.8.8 Script supervision — what is citable and what is not

The guild bodies (IATSE Local 871, the LA Script Supervisors Network) **publish
no sample forms** — a verified negative. The citable material is in published
textbooks.

**Who decides a circled take** — Elkins, *The Camera Assistant's Manual* 5th ed.:
*"**After each setup, the Script Supervisor will tell you which takes are to be
circled. These are the takes that the Director likes and wants to consider for
use in the editing of the film**... The circled takes are called good (G) takes.
The takes on the report that have not been circled are called no good (NG)."*

That is the authority chain — **Director decides → Script Supervisor relays →
2nd AC marks the camera report** — and it confirms §A.2's claim that circling is
an *editorial selection* recorded by a different person from the technical
assessment.

**Scene lettering**, Elkins, and this is the most useful single quote in the
section: *"When shooting a portion of a scene or a pick-up of action within a
scene, a letter is usually added to the scene number... **Some letters that are
not usually used for slating scene numbers are I, O, Q, S, and Z, which can
resemble numbers when written hurriedly. The letter I resembles the number one, O
and Q resemble the number zero, S resembles the number five, and Z resembles the
number two.**"*

Five excluded letters — **I, O, Q, S, Z** — with the reason. A slate-designation
generator that increments A, B, C… must skip exactly those. Elkins also documents
the US/UK split: *"In Britain it is common practice to not use scene numbers on
the slate... they are written as shot numbers, and the first shot on the first day
of filming is shot number 1."* — matching iXML's `<SCENE>` note (§A.8.1).

**The lined script**, Musburger & Ogden, *Single-Camera Video Production* 6th ed.:
*"♦ **Solid vertical lines** on the script indicate what each shot covers.
♦ **Squiggly lines** indicate off-camera dialogue or action not seen in that
particular shot. ♦ Shots are labeled with the scene number and an alpha
designation... The 'A' tag is typically the Master Scene shot and runs the length
of the scene."*

⚠️ Two fetched sources **disagree** on the squiggle: Musburger & Ogden say
*off-camera / not seen in this shot*; practising script supervisors define it as
*whether the actor's face is visible*. **No guild-published normative definition
exists.** If CINE-OMNI ever renders a lined script it must pick one and document
which.

⚠️ **`false_start` and `series` takes are `UNVERIFIED` as *film-production*
definitions.** The two standard references (Miller's glossary; Cybulski's *Beyond
Continuity*, which reportedly has a section by that exact name) could not be
fetched. §A.2's descriptions of them are therefore working definitions, not
citations — though iXML's `FALSE_START` in the `TAKE_TYPE` dictionary at least
confirms the term is standard vocabulary. `pickup` is verified only in the
*within-scene* sense (Elkins, above); the *reshoot-after-principal-photography*
sense is `UNVERIFIED`.

⚠️ **"MOS" etymology is folklore and the published sources say so.** Elkins:
*"**The Hollywood legend says** that the term MOS came from a German director who
could not say 'without sound.'... The literal translation of the term is **minus
optical sound**."* Do not put an etymology in a tooltip. The *paperwork*
convention is solid: *"If the shot was done with sync sound, write S in the
column for sync, and if it was done without sound, write M for MOS."*

---

## B. Dailies and DIT

### B.1 The job, stated as an invariant

A DIT's entire job reduces to one sentence: **get every frame off a reusable
card onto enough independent, verified copies that the card can be formatted.**
Everything else — proxies, LUTs, burn-ins, selects, editorial handoff — is
downstream convenience. The card format is irreversible, so the verification
question is not academic; it is the only question.

The industry rule of thumb is **3-2-1**, and it has a citable source. From
*"Data Backup Options"* by Paul Ruggiero and Matthew A. Heckathorn, *"© 2012
Carnegie Mellon University. Produced for US-CERT, a government organization"*,
hosted by CISA:

> *"To increase your chances of recovering lost or corrupted data, follow the
> 3-2-1 rule: **3 – Keep 3 copies of any important file: 1 primary and 2
> backups. 2 – Keep the files on 2 different media types to protect against
> different types of hazards. 1 – Store 1 copy offsite (e.g., outside your home
> or business facility).**"*

Two corrections to the folklore around it. **US-CERT did not originate the
rule** — that document's own footnote 1 credits *"Krogh, Peter. The DAM Book:
Digital Asset Management for Photographers, 2nd Edition, **p. 207**. O'Reilly
Media, 2009"*, and the wording also appears under Krogh's byline on the ASMP
dpBestflow site. And **it is not "ST19-001"** — that identifier appears nowhere in
the document.

Krogh's own formulation adds a caveat worth keeping: *"While 3-2-1 storage is
the ideal arrangement, it's not always possible... Best practices, however, still
require 3 copies and some physical separation between the copies."* — i.e. the
"2 media types" leg is the soft one; three copies and physical separation are
not.

The model consequence: **"verified" is a property of a *copy*, not of the
media.** A `media.material` is not verified; a `media.copy` on a particular
volume is. "Safe to format the card" is a *predicate over the set of copies*:
at least N verified copies on at least M distinct volumes, at least one offsite.
That predicate must be a function the system can evaluate and show, not a human
judgement call.

### B.2 The verification state machine

```
                    ┌───────────┐
                    │ discovered│  a file exists on the source; nothing copied
                    └─────┬─────┘
                          │ begin_copy
                    ┌─────▼─────┐
              ┌─────│  copying  │─────┐  temp name in the destination directory
              │     └─────┬─────┘     │
   interrupt  │           │ bytes written, fsync(file), fsync(dir), rename
              │     ┌─────▼─────┐     │ io error
              │     │  copied   │     │
              │     └─────┬─────┘     │
              │           │ begin_verify (RE-READ from the destination)
              │     ┌─────▼─────┐     │
              │     │ verifying │     │
              │     └──┬─────┬──┘     │
              │ digest │     │ digest │
              │  equal │     │ differ │
              │   ┌────▼─┐ ┌─▼──────▼─┐
              └──▶│      │ │  failed  │◀── later re-verification finds a mismatch
                  │verified│ └──────────┘        (bit rot; verified is not permanent)
                  └────────┘
```

Legal transitions, and only these:

| From | To | Trigger |
|---|---|---|
| `discovered` | `copying` | a copy job starts |
| `copying` | `copied` | all bytes written, file fsynced, directory fsynced, temp renamed to final |
| `copying` | `failed` | I/O error, cancellation, or process death (detected on restart) |
| `copied` | `verifying` | a verification pass starts |
| `verifying` | `verified` | the full contract in §B.3 holds |
| `verifying` | `failed` | any part of the contract fails |
| `verified` | `failed` | a later re-verification pass finds a mismatch |
| **anything** | **`verified`** | **forbidden except from `verifying`** |

There is deliberately no edge from `failed` back to `verified`. A failed copy is
re-copied from `discovered`; it is never "re-marked" good.

**This is corroborated by the standard, not just asserted.** ASC MHL's closed
`action` enumeration is `original | verified | failed`, and spec §5.6.4 requires
that *"Only hashes labeled as either `original` or `verified` can be used for
verification. Hashes labeled as `failed` cannot be used."* — the same absorbing
`failed`. It also settles a case §B.2 might have got wrong: *"Verification is
also considered unsuccessful if a file is missing in the managed data set."* So
for the purposes of *releasing a card*, an absent file is `failed`, even though
"offline" is otherwise a perfectly benign editorial state
([`09`](09-editorial-media-model.md) §3.1). The two senses of "missing" must not
be merged. See §B.7.3.

### B.3 What must be true before `verified` — the testable contract

`verified` is not a flag anyone may set. It is a *conclusion* with named
premises, and every premise is recorded on the node so the conclusion can be
audited and re-derived. Rules 1–9 were derived from first principles; rule 10
was taken from the ASC MHL reference implementation (§B.7.3), and is the one
that would not have been invented:

1. **Algorithm is a whole-file digest.** A head+tail+length fingerprint can never
   satisfy verification. (This is F2: today's `ContentHash::verify` violates it.)
2. **The source digest was computed from the bytes actually read from the
   source**, during the copy, not from a manifest supplied by someone else. (A
   supplied manifest can be *additionally* checked, and disagreement is itself a
   finding — but it cannot substitute for reading the source.)
3. **The destination digest was computed by re-reading the destination file after
   `fsync`**, not from the buffer that was written. Hashing the write buffer
   verifies RAM, not storage, and will happily "verify" a copy that the drive
   never committed. *This is the single rule that most implementations get wrong
   and the one that makes an incomplete copy masquerade as verified media.*
4. **Byte lengths are equal**, and both are recorded independently of the digest.
5. **The digests are equal and were produced by the same algorithm id.** Comparing
   an xxHash to a SHA-256 must be a type error, not a `false`.
6. **The result carries `verified_at`, the algorithm id, both digests, both
   sizes, and the verifier version.** Any of these absent ⇒ the state is not
   `verified`.
7. **No path exists during the copy that a later scan could mistake for finished
   media.** Write to a temp name, `fsync` the file, `fsync` the containing
   directory, then `rename` — atomic within a filesystem on POSIX. A crash leaves
   a temp file, never a plausible-looking short file at the final path.
8. **A `copying` node with no live job is `failed`, not `copied`.** On restart,
   temp files are deleted and interrupted nodes are transitioned to
   `failed(interrupted)`.
9. **`verified` decays.** `verified_at` is stored so re-verification can be
   scheduled; `verified` means "was verified at time T by algorithm A", never
   "is correct now".
10. **A newly-introduced digest algorithm may be recorded as `verified` only in a
    pass where an already-recorded algorithm verified.** Otherwise "upgrading"
    a file from SHA-256 to BLAKE3 would launder an unverified file into a
    verified one under the new algorithm. Taken verbatim from the ASC MHL
    reference implementation (`history.py::_validate_new_hash_list`, §B.7.3);
    it is the rule that makes R8's digest migration safe.

**The page cache is the trap inside rule 3.** Re-reading a file you have just
written usually gets served from the kernel's page cache — the same RAM you just
wrote from. A verification pass that does that has re-verified memory a second
time and proved nothing about the disk, while *looking* exactly like a
read-back. The measurement in §B.4 shows the tell: the "verify read XXH3-64"
pass ran at **1 859 MiB/s** on a device whose copy path managed 154 MiB/s. That
is not a disk read.

Two acceptable mitigations, both of which must be a deliberate, named choice
rather than an accident:

* advise the kernel to drop the file's pages before the read-back
  (`posix_fadvise(POSIX_FADV_DONTNEED)` on the destination fd, or open the
  read-back with `O_DIRECT`); or
* separate the passes in time and volume so eviction is genuine — verify a card
  after the whole card is copied, not per file.

Hash-while-copying (one read) and verify-by-read-back (two reads) are therefore
**not interchangeable**, and this is the reason the industry's careful tools do
the second read. Rule 3 should be stated in the code as *"the destination digest
must come from bytes the kernel fetched from the device"*, with the fadvise call
as its implementation and a test that fails if it is removed.

**Enforcing it in Rust rather than trusting it.** Make the conclusion
unconstructible except through the verifier:

```rust
/// A whole-file digest. `Fingerprint` (head+tail+length) is a *different type*
/// and does not convert into this one.
pub struct FullDigest { alg: DigestAlg, bytes: Vec<u8>, len: u64 }

/// Only obtainable from `verify`. No public constructor, no `Default`,
/// no `Deserialize` that bypasses the check.
pub struct Verified { source: FullDigest, dest: FullDigest, at: i64, verifier: &'static str }

impl Copied {
    /// Re-reads `dest` from disk. Consumes `self`, so a `Copied` cannot be
    /// verified twice into two different conclusions.
    pub fn verify(self, dest: &Path) -> Result<Verified, VerifyFailure>;
}
```

Because `Verified` cannot be constructed any other way, "mark it verified" is not
expressible in the type system. The graph writer that sets the `copy_state`
attribute takes a `&Verified`.

**The tests that make it a contract, not a comment:**

| Test | Asserts |
|---|---|
| `truncated_copy_is_never_verified` | destination short by 1 byte ⇒ `failed` |
| `corrupt_middle_block_is_detected` | flip one bit at the midpoint ⇒ `failed` (this is the test that fails today, F2) |
| `destination_digest_is_read_back_from_disk` | mutate the destination between write and verify ⇒ `failed`; proves the digest is not taken from the write buffer |
| `interrupted_copy_leaves_no_final_path` | kill mid-copy ⇒ no file at the final path, one temp file |
| `interrupted_copy_never_reports_copied` | restart ⇒ node is `failed(interrupted)` |
| `fingerprint_cannot_satisfy_verification` | a `Fast`/fingerprint digest is a compile error where a `FullDigest` is required |
| `cross_algorithm_comparison_is_a_type_error` | compile-fail test |
| `card_is_not_releasable_below_three_two_one` | the "safe to format" predicate is false with two copies on one volume |
| `verification_failure_is_recorded_not_silent` | a `failed` transition appears in the op log with the mismatching digests |
| `failed_is_absorbing` | no transition sequence reaches `verified` from `failed` — the ASC MHL rule (§B.7.3) |
| `new_algorithm_requires_a_verifying_pass` | adding BLAKE3 to a file that did not verify under its existing digest is refused (rule 10) |
| `page_cache_is_dropped_before_read_back` | the fadvise/O_DIRECT call is present; removing it fails the test |

The last one matters: because every state transition is a UCG op, **the op log is
the chain of custody**. That is not extra work; it falls out of the existing
architecture.

### B.4 Checksums — measured

Measured on this machine, in-memory over a 256 MiB pseudo-random buffer,
single-threaded, `--release`. Reproduction source is in this document's appendix
(§B.10). The benchmark was run **twice**: once while four background research
processes were saturating the machine, and once on a quiet machine. Both runs
are reported, because the difference is instructive — the *ordering* is
completely stable and the *absolute* numbers are not.

| Algorithm | Crate | Quiet: best | Quiet: mean | Loaded: best | Cryptographic? |
|---|---|---|---|---|---|
| CRC32 | `crc32fast` 1.5.1 | 7 280 MiB/s | 6 784 MiB/s | 6 514 MiB/s | no (error detection only) |
| XXH64 | `xxhash-rust` 0.8.18 | **6 716 MiB/s** | 5 690 MiB/s | 4 716 MiB/s | no |
| XXH3-64 | `xxhash-rust` 0.8.18 | **5 334 MiB/s** | 4 579 MiB/s | 5 020 MiB/s | no |
| XXH3-128 | `xxhash-rust` 0.8.18 | **5 111 MiB/s** | 3 847 MiB/s | 4 845 MiB/s | no |
| BLAKE3 | `blake3` 1.8.7, **`pure` feature** | **1 679 MiB/s** | 1 332 MiB/s | 1 033 MiB/s | yes |
| MD5 | `md-5` 0.10.6 | 357 MiB/s | 350 MiB/s | 112 MiB/s | **broken** for collisions |
| SHA-1 | `sha1` 0.10.7 | 292 MiB/s | 220 MiB/s | 86 MiB/s | **broken** for collisions |
| SHA-256 | `sha2` 0.10.9 | **123 MiB/s** | 119 MiB/s | 34 MiB/s | yes |

CPU: `Intel(R) Core(TM) i3-10110U @ 2.10GHz`, 4 logical cores. Relevant CPU
flags present: `aes`, `avx2`, `sse4_2`. **`sha_ni` is absent** — this machine has
no SHA hardware acceleration, which is why SHA-256 is slow here; a machine with
SHA extensions would be several times faster. That is itself a finding: the
SHA-256 figure is *not* portable, whereas the xxHash figure is roughly
data-path-bound everywhere.

Corroboration: Phase 2 independently measured `ContentHash::full` (SHA-256) at
**93 MiB/s** on the 5.52 MiB reference asset
([`../PHASE-2-MEDIA-ENGINE.md`](../PHASE-2-MEDIA-ENGINE.md) §12) — the same order
as the 123 MiB/s quiet figure here, from a different harness.

**The ratios that decide the design**, from the quiet run:
**XXH64 is 55× SHA-256. BLAKE3 (hobbled, pure-Rust) is 14× SHA-256 and still
cryptographic. MD5 is 3× SHA-256 — i.e. it buys almost nothing over SHA-256
while being collision-broken, which is why it is the worst available choice
rather than a compromise.**

`blake3` was built with the **`pure`** feature because its default build compiles
`c/blake3_sse2_x86-64_unix.S` through `cc-rs`, and that **failed** under this
machine's `zig cc` shim (build error captured during this research). So the
1 033 MiB/s figure is BLAKE3's *portable Rust* path with no SIMD assembly — its
assembly path is substantially faster, and that figure is **`UNVERIFIED` here**.
Two consequences: BLAKE3 is faster than SHA-256 by 14× even with one hand tied,
and *shipping* BLAKE3 on this toolchain currently means either the `pure` feature
or fixing the C build.

**End-to-end offload measurement**, 1 GiB file, copy from and to the same
filesystem (`/dev/sda2`), page cache warm, same loaded machine:

| Operation | Time | Throughput |
|---|---|---|
| copy only, no hash | 6.65 s | 154 MiB/s |
| copy + XXH3-64 | 7.86 s | 130 MiB/s |
| copy + SHA-256 | 22.03 s | 46 MiB/s |
| copy + BLAKE3 (pure) | 5.10 s | 201 MiB/s |
| verify pass, read + XXH3-64 | 0.55 s | 1 859 MiB/s |
| verify pass, read + SHA-256 | 27.54 s | 37 MiB/s |

(The BLAKE3 row being *faster than copy-only* is page-cache noise on a loaded
machine, and is reported rather than hidden; the ordering between SHA-256 and the
rest is far outside the noise.)

**The decision this supports.** The verification pass — re-reading the
destination, which contract rule 3 makes mandatory — costs **0.55 s/GiB with
XXH3 and 27.5 s/GiB with SHA-256** on the loaded run — a 50× difference, and
consistent with the 55× in-memory ratio from the quiet run. On a modest 1 TB shooting
day that is ~9 minutes versus ~7.8 hours. Whole-file verification with SHA-256 is
not merely slower; it is slow enough that people skip it, which is how unverified
media reaches an edit.

**Correctness of the fast path — verified, not assumed.** `xxhash-rust` produces
reference-identical values, which matters because manifests are exchanged with
other tools:

| Vector | Computed here | Reference |
|---|---|---|
| `XXH64("", seed 0)` | `0xef46db3751d8e999` | matches the published reference value |
| `XXH3_64("")` | `0x2d06800538d394c2` | matches |
| `XXH3_128("")` | `0x99aa06d3014798d86001c324468d497f` | matches |
| `MD5("abc")` | `900150983cd24fb0d6963f7d28e17f72` | RFC 1321 test suite |
| `BLAKE3("abc")` | `6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85` | matches published test vectors |

The reference values are stated from the algorithms' published test vectors;
re-fetching those pages to quote them verbatim is **pending** and the "Reference"
column is therefore `UNVERIFIED` as a *citation*, though the agreement between
four independent implementations' published constants and this build is itself
meaningful.

**Corroboration from the projects' own published benchmarks.** Both were fetched.

xxHash's README (machine stated verbatim: *"an Intel i7-9700K cpu... Ubuntu x64
20.04... compiled with `clang` v10.0 using `-O3`"*):

| Hash | Width | Bandwidth |
|---|---|---|
| XXH3 (SSE2) | 64 | **31.5 GB/s** |
| XXH128 (SSE2) | 128 | 29.6 GB/s |
| *RAM sequential read* | — | *28.0 GB/s* |
| XXH64 | 64 | 19.4 GB/s |
| SHA1 | 160 | 0.8 GB/s — *"Cryptographic but broken"* |
| MD5 | 128 | 0.6 GB/s — *"Cryptographic but broken"* |

with the project's own caveat: *"some algorithms feature _faster than RAM_
speed... they can only reach their full speed potential when input is already in
CPU cache (L3 or better). Otherwise, they max out on RAM speed limit."*

BLAKE3's chart data (from the benchmark script behind the README's SVG;
provenance comment verbatim: *"an AWS c5.metal instance, with an Intel Cascade
Lake-SP 8275CL processor... MD5, SHA-1, SHA-2, and SHA-3 are all provided by
OpenSSL"*), single-threaded, 16 KiB input: **BLAKE3 6 866 MiB/s**, BLAKE2b 1 312,
SHA-1 1 027, BLAKE2s 876, MD5 740, SHA-512 720, **SHA-256 484**, SHA3-256 394.

Both tables reproduce the ordering measured here. The absolute numbers are
higher, as expected on server parts; the *ratios* are what the design rests on.

> **⚠️ The SHA-256 figure is hardware-dependent, and this changes R8.** Intel's
> SHA Extensions add `sha256msg1`, `sha256msg2` and `sha256rnds2`; Intel's own
> kernel patch submission reports *"up to 3.6x speedup over existing SSSE3
> enhanced routines... on Broxton platform"*. This machine has **no `sha_ni`
> flag**, and an independent `openssl speed` run here gives SHA-256 ≈ **162 MB/s**
> at 16 KiB — the same order as the 123 MiB/s measured through `sha2`, and it
> also shows SHA-256 is *slower than SHA-512* on 64-bit hardware without the
> extension.
>
> So: on a machine without SHA-NI, SHA-256 is ~40× slower than XXH3 and the case
> for a fast transfer digest is overwhelming. On a SHA-NI machine the gap
> narrows substantially. **The honest conclusion is that the identity-digest
> choice should be a capability-gated decision, not a hardcoded constant** — the
> same shape as `cine-media`'s existing `probe::is_decodable` gate. The specific
> "with SHA-NI" throughput number is `UNVERIFIED`; no such CPU was available.

Two notes on the cryptanalysis, because "broken" is doing real work in the table
above and both claims were checked against the primary papers:

* **MD5** — Wang, Feng, Lai and Yu, *"Collisions for Hash Functions MD4, MD5,
  HAVAL-128 and RIPEMD"*, IACR ePrint 2004/199. The load-bearing clause is not
  the speed but the generality: *"On IBM P690, it takes about one hour to find
  such M and M′... **Moreover, our attack works for any given initial value.**"*
  That is what distinguishes it from earlier chosen-IV pseudo-collisions and is
  why MD5 is unusable where an adversary is in the threat model.
* **SHA-1** — Stevens, Bursztein, Karpman, Albertini and Markov, *"The first
  collision for full SHA-1"*, IACR ePrint 2017/190: *"the computational effort
  spent is equivalent to 2^63.1 calls to SHA-1's compression function, and took
  approximately 6,500 CPU years and 100 GPU years... still more than 100,000
  times faster than a brute force search."*
  ⚠️ **Do not cite `shattered.io`** — the domain no longer hosts that work and
  now serves unrelated auto-generated content. Cite the ePrint paper.

Neither matters for detecting *accidental* corruption, which is why ASC MHL
still permits both (§B.7.6). Both matter for content identity, which is the
second reason the two jobs need two algorithms.

**Collision reasoning, stated numerically rather than hand-waved.** For a
64-bit digest and *n* files, the birthday collision probability is
approximately `n²/2^65`. At n = 10⁶ files (a very large production) that is
`10¹²/3.69×10¹⁹ ≈ 2.7×10⁻⁸`. At n = 10⁷ it is `2.7×10⁻⁶`. For *accidental*
corruption — the actual threat model of a card offload — this is far below the
probability of undetected error anywhere else in the chain. For *content
identity across a project's whole history*, where a collision silently merges two
different takes into one entity, 64 bits is not the right answer and a 128-bit or
256-bit digest is.

This is very likely why the ASC MHL reference implementation defaults to
**XXH128** rather than XXH64 (§B.7.6) despite both being in the permitted
vocabulary — and it is a second, independent reason to follow that default
rather than pick XXH64 for its marginal speed advantage. At 128 bits the
birthday term is `n²/2^129`, which is not a number anyone needs to reason about
again.

That is the same Fast/Full split the code already has, drawn one level up: **the
algorithm follows the question, not the file.**

### B.5 F2 reproduced — `Fast` verification passes a corrupt file

`crates/cine-media/src/identity.rs`:

```rust
pub fn verify(&self, path: &Path) -> Result<()> {
    let actual = if self.is_full() {
        ContentHash::full(path)?
    } else {
        ContentHash::fast(path)?      // ← head + tail + length only
    };
    ...
}
```

`fast()` hashes the length, the first `FINGERPRINT_CHUNK` (1 MiB) and the last
1 MiB. A file larger than 2 MiB whose interior is corrupted has the same length,
the same head and the same tail, so `verify()` returns `Ok(())`.

**Executed, not argued.** A 4 MiB file, one bit flipped at offset 2 MiB:

```
=== F2: does ContentHash::verify() catch interior corruption? ===
  file size              : 4194304 bytes (4 MiB)
  before corruption: Fast::verify ok = true, Full::verify ok = true
  after flipping ONE BIT at offset 2 MiB:
    Fast::verify() -> Ok(())   ** CORRUPTION NOT DETECTED **
    Full::verify() -> Err (detected)
  RESULT: CONFIRMED - a Fast hash 'verifies' a corrupt file; only Full detects it
  after truncation to 3 MiB: Fast::verify() -> Err (detected: length is hashed)
```

The truncation line is worth reading carefully: `Fast` *does* catch truncation,
because the length is hashed. So the failure mode is not "fingerprints are
useless" — it is the narrower and more dangerous one that a fingerprint catches
the obvious corruption and misses the silent kind, which is exactly the profile
of a check that gets trusted. The Phase 2 document's own framing — "`Full` … is
for conform and verification, where being certain matters more than being quick"
— is the correct policy; the API does not enforce it.

Source for the reproduction is in §B.10.

Fix, in order of preference:
1. Rename the fingerprint path `matches_fingerprint()` and make `verify()`
   require a `Full` hash, returning an error when handed a `Fast` one.
2. Type-split as in §B.3 so it cannot be called wrongly.

### B.6 F1 reproduced — a script edit can delete takes

The chain, all three links read from the source tree:

* `crates/cine-logistics/src/graph.rs`, `write_take_metadata`:
  `tx.link(shot, id, EdgeKind::Contains)?;` — a take is a `Contains`-child of a
  shot.
* `crates/cine-logistics/src/graph.rs`, `write_shot_list`:
  ```rust
  for stale in children_of_kind(tx.graph(), list, kinds::LOGISTICS_SHOT) {
      if !keep.contains(&stale) { tx.delete(stale)?; }
  }
  ```
* `crates/ucg-core/src/tx.rs`, `Tx::delete`:
  ```rust
  let kids = self.graph.children(id);
  for k in kids { self.delete(k)?; }
  ```

**Executed, not argued.** Create a scene and a previs camera; write a shot list
with one shot; capture three takes against that shot; regenerate the shot list
without that camera — which is exactly what `LogisticsReactor` does when the
camera is removed:

```
=== F1: does regenerating a shot list delete captured takes? ===
  shot node present      : true
  takes present          : 3
  shot present after     : false
  takes present after    : 0
  ops in that one delta  : 4
  RESULT: CONFIRMED - regenerating derived data DELETED 3 captured takes
```

Four ops: one `DeleteNode` for the shot and three for the takes, in a single
delta, inside a single undo group, with no user involvement.

`LogisticsReactor` (`crates/cine-pipeline/src/reactors.rs`) runs `write_shot_list`
on any change to `script.scene` or `previs.camera`. Shot ids are
`EntityId::derived(camera, kinds::LOGISTICS_SHOT)` — keyed on the **previs
camera**. So removing a previs camera, or a scene edit that causes the shot list
to be regenerated without that camera, deletes the shot **and every take under
it**, in a reactor cascade, inside a single undo group.

Two independent defects meet here:

1. **Identity.** A shot's identity is derived from a previs camera, so a shot has
   no existence independent of previs. On set, shots exist that were never
   previsualised.
2. **Ownership.** Captured data hangs beneath derived data, so regenerating the
   derived data destroys the captured data.

The general rule, proposed for `docs/MODULE-CONVENTIONS.md`:

> **No reactor may delete a node that has captured descendants.** Derived
> writers reconcile by *updating* and by *marking absent*, never by deleting,
> once a node has acquired authored or captured children. Deletion of captured
> data is a user action, never a cascade.

Mechanically the cheapest enforcement is a guard in `write_shot_list` plus a
regression test; the more durable one is an attribute (`captured = true`) that
`Tx::delete` refuses to cross without an explicit override.

### B.7 Manifests — ASC MHL, verified against the specification

**Verified.** Fetched and read in full: the ASC MHL Specification v1.0 (15 March
2022, 38 pp.) and Implementation Guidelines v1.0 (29 March 2023) from
<https://github.com/ascmitc/mhl-specification>; the reference implementation,
XSDs and generated example scenarios from <https://github.com/ascmitc/mhl>; the
pre-ASC MHL v1.1 XSD and real `.mhl` files from <https://mediahashlist.org/> and
<https://github.com/pomfort/mhl-tool>; and SMPTE ST 2114:2017 (C4 ID) from
<https://pub.smpte.org/latest/st2114/st2114-2017.pdf>.

#### B.7.1 Licence — and one important negative result

| Artefact | Licence | Evidence |
|---|---|---|
| `ascmhl` reference tool + `mhllib` + the XSDs | **MIT** | `LICENSE.md` is the MIT template, `Copyright (c) 2020 ASC`; `setup.py` classifier `License :: OSI Approved :: MIT License`; every module header `__license__ = "MIT"` |
| `pomfort/mhl-tool` (the *old* MHL reference impl) | **MIT** | `LICENSE`, `Copyright (c) 2016 Pomfort GmbH` |
| Avalanche-io `c4` (Go C4 implementation) | **Apache-2.0** | repo `LICENSE` |
| **The ASC MHL specification PDFs themselves** | **no licence and no copyright notice at all** | the `mhl-specification` repo contains exactly four files and **no LICENSE**; a grep for `copyright\|licen\|rights reserved\|creative commons` across the extracted text of **both** PDFs returned **zero hits** |

That last row is a real finding rather than a gap. The *format* is open and its
reference implementation is MIT, so implementing it is unproblematic. But the
specification document carries no grant, so **it must be treated as
all-rights-reserved for redistribution purposes**: read it, implement from it,
do not vendor it into this repository. (Contrast SMPTE ST 2114, which does carry
an explicit `Copyright © 2017 by THE SOCIETY OF MOTION PICTURE AND TELEVISION
ENGINEERS` — and is nonetheless freely downloadable from `pub.smpte.org`.)

#### B.7.2 The structure, exactly

Root, namespace, and a version skew worth knowing about: the *specification
document* is v1.0 but the *format* it defines is v2.0.

```xml
<hashlist version="2.0"
  xmlns="urn:ASC:MHL:v2.0"
  xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
  xsi:schemaLocation="urn:ASC:MHL:v2.0 ASCMHL.xsd">
```

Spec §6.4.1: *"The HashListType version attribute shall be set to the value
'2.0' (for distinguishing documents of this ASC MHL version 2 from the previous,
incompatible MHL version 1)."* The chain file uses a **different** namespace,
`urn:ASC:MHL:DIRECTORY:v2.0`, with root `<ascmhldirectory>`.

```
hashlist@version="2.0"
├── creatorinfo   (required)  creationdate, hostname, tool@version,
│                             author@email @phone @role (0..n), location?, comment?
├── processinfo   (required)  process (in-place | transfer | flatten),
│                             roothash? (content + structure), ignore? (pattern 1..n)
├── hashes                    choice of hash | directoryhash, 1..n
├── metadata?                 anyType
└── references?               hashlistreference 1..n  (path + c4, c4 mandatory)
```

A file entry. **Identity is the relative path; size and mtime are optional
attributes on the path element, not part of identity:**

```xml
<hash>
  <path size="5" lastmodificationdate="2020-01-15T13:00:00+00:00">Clips/A002C006_141024_R2EC.mov</path>
  <xxh64 action="original" hashdate="2020-01-16T09:15:00+00:00">0ea03b369a463d9d</xxh64>
</hash>
```

Multiple algorithms per file are sibling elements in a **schema-significant fixed
order** (`<sequence>`, not `<choice>`): `c4, md5, sha1, xxh128, xxh3, xxh64`.
Spec §6.5: *"One or more of the following elements shall be present: md5, sha1,
c4, xxh64, xxh3, xxh128."*

Path rules, spec §6.4.8, verbatim: *"case shall be preserved / whitespaces shall
be preserved / forward slash ('/') shall be used to separate components / the
path shall not start with forward slash / the most direct path shall be used,
e.g. '..' path segments are not allowed."* Uniqueness (§6.4.9): no two entries
may share a `path`.

#### B.7.3 `action` — the state machine, and it is our state machine

This is the single most useful thing in the specification for §B.2. The
enumeration is **closed and has exactly three values**
(`xsd/ASCMHL.xsd`, `ActionAttributeType`):

```xml
<enumeration value="original"/>
<enumeration value="verified"/>
<enumeration value="failed"/>
```

Spec §6.5.1, verbatim:

> * `original` indicates that the hash is the initial hash for a file or
>   directory within the ASC MHL History.
> * `verified` indicates that the hash was generated after a file was
>   successfully verified against a previously recorded hash in the ASC MHL
>   History. The verification is deemed successful when a hash generated using
>   one of the algorithms recorded in the ASC MHL History matches the
>   corresponding hash in the ASC MHL History.
> * `failed` indicates that the hash value has been created from a file but
>   verification against a previously recorded hash in the ASC MHL History
>   failed, i.e. the file was altered.

And spec §5.6.4, which is the verification contract stated by the standard
itself:

> **Only hashes labeled as either `original` or `verified` can be used for
> verification. Hashes labeled as `failed` cannot be used. Verification is
> successful if the hashes match, and unsuccessful otherwise. Verification is
> also considered unsuccessful if a file is missing in the managed data set.**

Four things follow, and every one of them corroborates §B.2/§B.3 from an
independent direction:

1. **`failed` is absorbing.** The Implementation Guidelines §2.7.1: *"any hash
   labeled as `failed` should not be used. This ensures that files are only ever
   verified against their originally-calculated hashes."* That is exactly §B.2's
   missing `failed → verified` edge — the standard removes it too.
2. **A missing file is a verification failure, not a separate category.** §B.2's
   `failed` state absorbs "offline" for the purpose of releasing a card, which is
   correct even though offline is otherwise a benign state
   ([`09`](09-editorial-media-model.md) §3.1). *Verification* of an absent file
   is a failure; *editing* around an absent file is not.
3. **`verified` is per (file, algorithm), and the algorithm must be one already
   in the history.** §5.6.1.3: *"integrity verification needs to be performed
   based on a hash algorithm used to create previous hash records."* That is
   §B.3's rule 5 — cross-algorithm comparison is meaningless — stated as a
   normative requirement.
4. **There is no `new`.** The reference implementation uses `"new"` as a
   *transient in-memory marker only*, and `history.py::_validate_new_hash_list`
   refuses to persist it unless some already-recorded algorithm verified in the
   same pass:

   ```python
   if required_hash_entry.action != "verified":
       raise AssertionError("hash entry for new hash not verified", ...)
   hash_entry.action = "verified"
   ```

   In other words: **you may add a stronger digest to an existing file only in a
   pass where the file verified under a digest you already had.** That is a rule
   CINE-OMNI needs verbatim for R8's SHA-256 → BLAKE3 migration, and it is not
   obvious enough to have invented.

`action` is *required* on file hashes (§6.5.1) and, in the reference
implementation's actual output, absent on directory hashes — though the spec's
own §6.5.2 examples show it there. Accept both; emit per the implementation.

#### B.7.4 Directory hashes — two per directory, and they answer different questions

Every directory carries a **pair**, both required by `DirectoryHashType`:

```xml
<directoryhash>
  <path lastmodificationdate="...">Clips</path>
  <content>  <xxh64 hashdate="...">4c226b42e27d7af3</xxh64></content>
  <structure><xxh64 hashdate="...">906faa843d591a9f</xxh64></structure>
</directoryhash>
```

Implementation Guidelines §2.5.1/§2.5.2, verbatim, and the distinction is the
whole point:

> [content] Changing the content of a file in the directory changes the content
> hash value. / Changing the name of a file or subfolder at that directory level
> does not change the content hash value. / Moving a file to another folder in
> the directory hierarchy changes the content hash value.
>
> [structure] Changing the name of a file or subfolder at that directory level
> **changes** the structure hash value.

So `content` answers "are the bytes all still here?" and `structure` answers "is
the layout still what it was?" — two questions that a single directory digest
conflates, and that a card offload genuinely needs answered separately (a
renamed clip is a different problem from a corrupted one).

The combining function, Appendix G, verbatim:

> 1. Given a list of hashes, reset the hash-generator to a clear state.
> 2. **Sort the list of hashes by hash value lexicographically.**
> 3. For each hash: encode the hash value into its byte representation; write
>    the bytes to the hash-generator.
> 4. Produce a digest. 5. Decode into its string value.

Two implementation traps, both worth recording because they are interop
landmines: the sort is over the **hash strings**, before decoding to bytes; and
the name encoding used by `structure` (UTF-8 of the **basename only**, name bytes
then hash bytes) appears **only in the implementation**, not in the specification
text.

#### B.7.5 The history, the chain, and how far its tamper-evidence actually goes

Layout, from the repository's own generated scenarios:

```
Reels/
├── ascmhl/
│   ├── 0001_Reels_2020-01-17_143000Z.mhl
│   └── ascmhl_chain.xml
└── A002R2EC/
    ├── ascmhl/
    │   ├── 0001_A002R2EC_2020-01-16_091500Z.mhl
    │   ├── 0002_A002R2EC_2020-01-17_143000Z.mhl
    │   └── ascmhl_chain.xml
    └── Clips/…
```

Naming, §6.3: `<numbering> "_" <foldername> "_" <date> "_" <time> ".mhl"`, with
`<numbering>` a 4-digit sequential number from 1, `<date>` `YYYY-MM-DD` and
`<time>` `HHMMSSZ`. §5.2: *"Once created, ASC MHL Manifest files are immutable
and shall not be altered."*

The chain file is a flat list of `(sequencenr, path, c4)`:

```xml
<ascmhldirectory xmlns="urn:ASC:MHL:DIRECTORY:v2.0">
  <hashlist sequencenr="1"><path>0001_A002R2EC_….mhl</path><c4>c43qQABr…</c4></hashlist>
  <hashlist sequencenr="2"><path>0002_A002R2EC_….mhl</path><c4>c45kfAU9…</c4></hashlist>
</ascmhldirectory>
```

with §7.4.1 requiring the sequence be dense from 1.

**And here is the consequential limitation.** The chain gives immutability by
convention, a recorded C4 per generation, and a dense sequence. It does **not**
give a hash link between generations — generation N does not commit to
generation N−1 — and ASC MHL v2 has **no signature** (the *old* MHL v1.1 XSD did
allow an optional `<xs2:Signature>` from `http://www.w3.org/2000/09/xmldsig#`;
v2 dropped it). The chain file is itself unhashed and unsigned, so it is an
**unprotected trust root**: anyone who can rewrite `ascmhl_chain.xml` can rewrite
history without detection.

That is a direct argument for R18. CINE-OMNI's op log, with §C.5's `parents` +
`content_hash`, is a genuine Merkle DAG — strictly stronger than the ASC MHL
chain. The right relationship is therefore: **the op log is the authoritative
chain of custody; ASC MHL is the interchange format we read and write at the
boundary.** Emitting conformant ASC MHL is an interop requirement; relying on it
for tamper-evidence would be a downgrade.

Nesting, §5.3.2: *"The hash record of a file or directory shall be recorded in
the ASC MHL History closest to it in terms of the file system hierarchy."*
Updates propagate downward into nested histories but *"do not propagate into
higher-level ASC MHL Histories"*, with the explicit consequence that *"the
references previously recorded in the higher level ASC MHL Histories are outdated
following such update."* **A stale `hashlistreference` C4 is expected and is not
a tamper signal** — a rule any validator must encode or it will cry wolf.

Renames, §5.6.6: a `previousPath` element is written *"only in the hash record
created as part of the renaming process and not repeated in subsequent ASC MHL
Manifest generations."* So path identity across a history is a **chain of
renames**, not a single key — which is one more reason CINE-OMNI's own identity
must be content-derived and not path-derived (F4, and
[`09`](09-editorial-media-model.md) §1.8).

#### B.7.6 The hash vocabulary — and what it means for our algorithm choice

Permitted formats, §6.5, quoted above: **`md5, sha1, c4, xxh64, xxh3, xxh128`**.
Glossary §4.6–4.11 binds them to RFC 1321, RFC 3174, SMPTE ST 2114 and the
xxHash specification respectively.

The reference implementation's own priority order and defaults
(`ascmhl/__version__.py`):

```python
# decreasing priority list for verification
ascmhl_supported_hashformats = ["md5","sha1","xxh128","xxh3","xxh64","c4"]
ascmhl_default_hashformat = "xxh128"
ascmhl_reference_hash_format = "c4"
```

Three decisions fall directly out of this:

1. **The industry's default is xxHash, and specifically XXH128.** That is
   independent corroboration of R7 from the standard's own reference tool, not
   just from the throughput measurement in §B.4.
2. **SHA-256 is not in the vocabulary. Neither is BLAKE3.** So CINE-OMNI's
   *identity* digest — SHA-256 today, BLAKE3 proposed (R8) — **cannot be
   expressed in an ASC MHL manifest at all.** This settles a question that §B.4
   left open: the transfer digest and the identity digest cannot be unified even
   in principle, because the interchange format will not carry the identity one.
   Two digests is not a compromise; it is forced.
3. **C4 is required, not optional, in two places** — `hashlistreference/c4` and
   the chain's `hashlist/c4` are typed as mandatory single elements. Writing
   conformant ASC MHL therefore requires a SHA-512 + base58 implementation
   regardless of what we use elsewhere.

Encoding, Appendix D — **and the appendix is wrong, which matters**. Its prose
says digests are "base64 [RFC4648]" and gives octet counts that contradict its
own examples (SHA-1 as "16-octet … (40 characters)"; XXH64 as "16-octet …
(32 characters)" beside a 16-hex-character example). The examples and the
reference implementation agree with each other and not with the prose:
**lowercase hex, no separators, XXH seeds of 0, big-endian**; only C4 is base58.
Implement to the examples.

#### B.7.7 C4 — SMPTE ST 2114:2017, read directly

Verified from the standard text, which is freely downloadable:

* **It is SMPTE ST 2114:2017, "Unique Digital Media Identifier (C4 ID)"**,
  approved 21 July 2017. **There is no IEEE standard number.** A grep of the
  full text for "IEEE" returns only `ISO/IEC/IEEE 9945:2009` (POSIX), cited for
  regex syntax. IEEE Xplore merely resells the SMPTE document.
* §5.2: *"A C4 Digest shall be a **64-byte SHA-512** message digest of the
  digital data unit, as specified in ISO/IEC 10118-3, and shall be interpreted as
  an unsigned big-endian integer."*
* §5.3: *"The C4 ID shall be a **90-character** string"* = the 2-character prefix
  `"c4"` plus *"an 88-character string"*, matched by
  `c4[1-9A-HJ-NP-Za-km-z]{88}`.
* §5.1: base58 over
  `"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"` — *"excluding
  the characters '0' zero, 'O' upper-case oh, 'I' upper-case, and 'l'
  lower-case el."*
* **Fixed length by left-padding with the base58 zero digit `1`**, confirmed by
  the standard's own test vector for the value 0:
  `c4111111…1111` (88 ones). Annex A: `LOOP WHILE LENGTH(result) < 88`. Pad,
  never truncate.
* §5.4 defines a **set/folder ID as a sorted binary Merkle tree with duplicate
  removal** — repeatedly pair adjacent sorted digests, concatenate to 128 bytes,
  re-digest.

⚠️ **§5.4 is *not* what ASC MHL uses for directory hashes.** ASC MHL Appendix G
is a flat sort-and-concatenate into one digest, with no tree and **no
deduplication**. Two different "hash of a set" algorithms live in the same
ecosystem and must not be cross-wired.

Being SHA-512-based, C4 inherits SHA-512's cost. On this machine SHA-256 measured
123 MiB/s (§B.4); SHA-512 is typically faster than SHA-256 on 64-bit hardware
without SHA extensions, but is still two orders of magnitude below xxHash. C4 is
therefore right where the standard puts it — as a **manifest-level reference
identifier over one small XML file**, not as a per-frame-of-rushes digest.

#### B.7.8 The old MHL (v1.1), because we will be handed these files

No XML namespace at all — which is how a parser must discriminate the two:

```xml
<hashlist version="1.0">
  <creatorinfo><name>…</name><username>…</username><hostname>…</hostname>
    <tool>mhl ver. 0.2.32</tool><startdate>…</startdate><finishdate>…</finishdate></creatorinfo>
  <hash>
    <file>file0.txt</file>
    <size>555</size>
    <lastmodificationdate>2016-04-18T14:45:37Z</lastmodificationdate>
    <xxhash64be>f61411669ed938e3</xxhash64be>
    <hashdate>2016-04-18T14:47:57Z</hashdate>
  </hash>
</hashlist>
```

Differences that a reader must handle, all verified against the v1.1 XSD and
real files from the `pomfort/mhl-tool` test corpus:

| | MHL v1.1 | ASC MHL v2.0 |
|---|---|---|
| Namespace | **none** | `urn:ASC:MHL:v2.0` |
| File identity element | `<file>` | `<path>` |
| size / lastmodificationdate | **mandatory child elements** | optional *attributes* on `<path>` |
| hashdate | mandatory child element | optional attribute |
| **Verification state** | **none — there is no `action` at all** | `original\|verified\|failed` |
| Signature | optional W3C XML-DSig `<xs2:Signature>` | **dropped** |
| Endianness | `<xxhash64>` *"Deprecated, little endian"*; `<xxhash64be>` big-endian | big-endian only |
| Oddities | `<null>` ("no hash, only file size verification"); XXH32 as a **decimal integer**; the XSD attribute is misspelled `referencehhashlist` | — |

The endianness split is not cosmetic: the same file appears in the test corpus as
`<xxhash64>e338d99e661114f6</xxhash64>` and
`<xxhash64be>f61411669ed938e3</xxhash64be>` — byte-reversed. Reading a v1 file
and assuming one convention silently produces "corruption" on every entry.

**They are not versions of one format.** Implementation Guidelines §2.9,
verbatim: *"Although ASC MHL is conceptually similar to the original Media Hash
List (MHL) specification … **ASC MHL is not a derivative of MHL and is not
backward compatible with it.**"*

#### B.7.9 What CINE-OMNI should do about it

| Decision | Rationale |
|---|---|
| **Read both** ASC MHL v2 and MHL v1.1; **write only ASC MHL v2** | v1 has no verification state to write into; writing it would be recording less than we know |
| Default digest **XXH128**, matching `ascmhl_default_hashformat` | interop with the reference tool by default, and it is the fastest of the strong options (§B.4) |
| Implement **C4** (SHA-512 + base58, left-pad with `1` to 88) | mandatory in `hashlistreference` and the chain; cheap because it only ever runs over small XML files |
| Map our `copy_state` onto `original \| verified \| failed` **exactly** | the vocabulary already is the state machine; inventing a fourth value would break the round trip |
| Encode `verified` only via the §B.3 contract | the standard requires the algorithm to be one already in the history; our type-level rule is strictly stronger |
| Adopt the "add a new algorithm only in a verifying pass" rule for the BLAKE3 migration (R8) | taken from `history.py::_validate_new_hash_list`; it is the correct answer and we would not have derived it |
| Treat the ASC MHL chain as **interchange, not as our audit trail** | it has no inter-generation hash link and no signature (§B.7.5); the op log's Merkle DAG is stronger |
| Emit both `content` and `structure` directory hashes | they answer different questions and both are required by the schema |
| Do **not** vendor the specification PDFs into this repository | no licence grant (§B.7.1) |
| Mirror `ASCMHL.xsd` locally | `ASCMHLDirectory.xsd` imports it over the network from `raw.githubusercontent.com`; a build must not depend on that |

### B.8 Proxies, LUTs, burn-ins and the editorial handoff

Every one of these is already a solved shape in this codebase and should not
acquire a second mechanism:

* **Proxies and transcodes** are `media.derivative` with `DerivedFrom` back to the
  source (`crates/cine-media/src/ucg.rs`), carrying a `recipe` string so a stale
  derivative is detectable when the recipe changes. That is correct and complete
  for dailies transcodes. The one addition dailies needs is that a
  *user-supplied* proxy must **not** get the `DerivedFrom` edge — that edge is
  precisely the licence to regenerate, and the system must not silently
  regenerate something a human made. ([`09`](09-editorial-media-model.md) §3.4
  makes the same point.)
* **LUTs and looks** are `color.lut` and the `Cdl` component
  (`crates/ucg-schema/src/components.rs`), already carrying ASC CDL
  slope/offset/power/saturation. A dailies look is a `color.grade`/`color.lut`
  referenced by the record, not baked into the identity of the media.
* **Burn-ins** are a property of a *derivative's recipe*, never of the material.
  A burned-in proxy is a different derivative from a clean proxy of the same
  material, and the recipe string already distinguishes them because
  `Derivative::entity_id()` includes the recipe.
* **Selects and editorial handoff** are the `circled`/`good` status from §A.2
  turned into a query. A "selects reel" is a saved query
  ([`09`](09-editorial-media-model.md) §8.2's `media.query`), not a copied set of
  files.
* **LTO / archive** is one more `media.copy` with a volume kind of `lto`, which
  is why "verified" being per-copy (§B.1) matters: the archive copy has its own
  verification with its own timestamp, and the 3-2-1 predicate reads it.

### B.9 "Safe to format" as a function, not a judgement

The one irreversible act in the whole pipeline is formatting a card. It should
therefore be gated by a predicate the system can evaluate, display, and refuse
on — never by a human's recollection of whether the second copy finished.

```rust
pub struct ReleasePolicy {
    pub min_verified_copies: usize,   // 3-2-1: 3
    pub min_distinct_volumes: usize,  //         2 (independent devices)
    pub require_offsite: bool,        //         1
    pub max_verification_age: Duration,
}

/// Every representation of every record of every take on this card must satisfy
/// the policy. A single unverified file makes the whole card unreleasable.
pub fn card_release_state(g: &Graph, card: EntityId, p: &ReleasePolicy) -> ReleaseState
```

Three properties this must have, each of which is a way real workflows fail:

* **It is `for_all`, not `exists`.** One file short of policy blocks the card.
  A per-file view that says "97 % complete" is how the other 3 % gets formatted.
* **It names what is missing.** `Blocked { file, reason }`, not `false`.
  Refusing without saying why guarantees the refusal gets overridden.
* **The offsite copy counts only if its own verification is recent enough.** An
  LTO copy verified eighteen months ago and never read since is a copy, not a
  verified copy — hence `max_verification_age` and hence §B.3's rule 9 that
  `verified` decays.

Archive (LTO or otherwise) needs nothing new: it is one more `media.copy` on a
`media.volume` whose `volume_kind` is `lto` and whose `location` is `offsite`.
Because verification is per copy, the tape's verification has its own timestamp
and its own digest, and the release predicate reads it like any other. The
alternative — a separate "archive" subsystem with its own notion of correctness
— is how the archive copy ends up being the one nobody ever checked.

**Editorial handoff** is an *export*, therefore derived, therefore recomputable:
an interchange file (ALE / AAF / FCPXML / OTIO, exact formats out of scope here)
generated from the take records and their bindings. It must carry the strongest
identity key available (§A.7 rank 1–2) so the receiving system can relink, and it
must never be the *only* place a fact lives.

### B.10 Reproduction

The measurements in §B.4 come from a standalone crate built against
`xxhash-rust 0.8.18`, `blake3 1.8.7` (feature `pure`), `md-5 0.10.6`,
`sha1 0.10.7`, `sha2 0.10.9`, `crc32fast 1.5.1`. Structure:

```rust
// 256 MiB xorshift-filled buffer; best-of-N wall time per algorithm.
bench("SHA-256", &data, 3, |d| { let mut h = sha2::Sha256::new(); h.update(d); .. });
bench("XXH3-64", &data, 5, |d| xxhash_rust::xxh3::xxh3_64(d));
// offload: BufReader/BufWriter 1 MiB chunks, hash-while-copying, then
// fsync + a separate re-read pass for the verification figures.
```

The F1 and F2 reproductions (§B.5, §B.6) are one small binary depending on
`ucg-core`, `ucg-schema`, `cine-logistics` and `cine-media` by path, run against
the working tree. Both printed `CONFIRMED`. The essential parts:

```rust
// F1 — regenerating derived data destroys captured data
engine.transact("shot list", |tx| { write_shot_list(tx, scene, &[entry])?; Ok(()) })?;
let shot = shot_id(camera);
engine.transact("takes captured on set", |tx| {
    for n in 1..=3 { write_take_metadata(tx, shot, n, &TakeMetadataSummary::default(), &[])?; }
    Ok(())
})?;
engine.transact("regenerated without that camera", |tx| { write_shot_list(tx, scene, &[])?; Ok(()) })?;
// -> shot gone, all three takes gone, 4 DeleteNode ops

// F2 — a fingerprint 'verifies' a corrupt file
let fast = ContentHash::fast(&path)?;          // 4 MiB file
flip_one_bit_at(&path, 2 << 20);
assert!(fast.verify(&path).is_ok());           // passes. It should not.
assert!(ContentHash::full(&path_before).verify(&path).is_err());
```

---

## C. Collaboration

### C.1 The constraint that decides everything

CINE-OMNI does not have a document that users co-edit. It has a **reactive
graph**: a user transaction commits a `Delta`, the engine runs reactors in
parallel, their plans commit further deltas at increasing `depth`, and the whole
cascade becomes one `UndoGroup`
(`crates/ucg-core/src/engine.rs`, `transact` → `cascade`).

Any collaboration design that ignores the cascade will fail immediately, because
**most of the ops in the log are not authored by a human.** A one-line dialogue
edit produces script ops, breakdown ops, shot-list ops, schedule ops, call-sheet
ops, timeline conform flags, audio placeholder ops and comp/colour ops. Shipping
all of that across a wire and merging it is both enormously wasteful and
semantically wrong: two sites that merged derived ops would be merging *answers*
instead of recomputing them.

### C.2 What the op log already gives us

From `crates/ucg-core/src/tx.rs`:

```rust
pub struct Delta {
    pub revision: u64,          // post-apply local revision
    pub label: String,
    pub ops: Vec<Op>,
    pub actor: String,          // "user id / machine id, for collaboration"
    pub lamport: u64,           // "Lamport-style logical clock for merge ordering"
    pub cause: Option<u64>,     // the revision that caused this one
    pub depth: u32,             // 0 = user action
    pub kind: DeltaKind,        // Normal | Undo | Redo
}
```

and `Op` is `CreateNode | DeleteNode | SetAttr | RemoveAttr | AddEdge | RemoveEdge`,
each carrying enough to invert itself. The doc comment already states the intent:
ops "carry enough information to be replayed on another machine (the
collaboration/persistence primitive)."

Four properties are genuinely valuable and rare:

1. **Append-only, serialisable ops.** `oplog/NNNN.jsonl`, one JSON delta per line
   (`crates/ucg-core/src/persist.rs`).
2. **Invertibility**, with `DeleteNode` carrying the node *and its edges*.
3. **Grouped undo that survives a process restart** — `rebuild_history` regroups
   deltas by `depth` and replays `Undo`/`Redo` kinds.
4. **`depth` distinguishes authored from derived ops.** This is the load-bearing
   one and, as far as the code shows, it is currently only used for undo grouping
   and cascade limits. It is worth much more than that (§C.6).

### C.3 Where it breaks under concurrency — precisely

**`SetAttr` carries `prev`, and `prev` is state-dependent.** Replaying a remote
`SetAttr { id, key, value, prev }` onto a local graph where that attribute has
since changed applies the right *value* but records a *wrong inverse*. `Op::inverse`
returns `SetAttr { value: prev }` — restoring a value that was never there
locally. Undo after a merge would therefore silently rewrite history to a state
that never existed on this site. This is the sharpest reason the current op log
cannot simply be replayed across sites: **the ops are invertible only against the
exact state they were generated on.**

**`Op::is_noop` and `Tx::set`'s idempotence are also state-dependent.**
`Tx::set` returns early when `prev == value`, so whether an op even *exists* is a
function of local state. Two sites will not produce identical op sequences for
identical user actions, which is fine for a merge design and fatal for a
"replicate the ops verbatim" design.

**`revision` and `cause` are site-local integers.** `cause: Option<u64>` points
at a revision number that means nothing on another machine. Any cross-site
causality needs a globally unique reference.

**Derived-id creates collide by design.** `EntityId::from_key` /
`EntityId::derived` are deterministic, so two sites that independently ingest the
same file or run the same reactor mint the *same* `EntityId` and both emit
`CreateNode` for it. That is exactly what you want for convergence, and it means
`CreateNode` must be idempotent on replay (create-if-absent), not an error.

**`DeleteNode` vs concurrent edit.** A remote delete carrying the node's old
attributes, applied after a local edit, loses the local edit with no trace. A
policy is required and none exists.

### C.4 How the industry actually does it — vendor-documented behaviour

Fetched from vendor documentation. No internals are asserted; only what the
vendor publishes about observable behaviour.

**DaVinci Resolve** (Reference Manual, chapters 194–195) is the most
sophisticated of the three, and its granularity is **three-tiered**:

> *"Multi-user collaborative workflow uses 'bin locking' to manage who has
> access to what... Using bin locking, multiple editors can simultaneously edit
> different timelines in different bins of the same project... Using clip
> locking, multiple colorists and compositing artists can work together in the
> same timeline... **One editor, one compositing artist, and one colorist can
> work together on the same clip in the same Timeline of the same project without
> conflict.**"*

The partition is **by discipline, not by region**: the editor owns the cut, the
colorist owns the grade, the compositor owns the Fusion comp, and they can hold
the *same clip* simultaneously because they touch disjoint attribute sets. The
conflict rule is stated plainly — *"a 'first come, first served' model"* — and
timeline-level editing is exclusive: *"The first collaborator that opens a
timeline is the only person that can make editorial changes... Other
collaborators... are 'locked out' of making changes to the Edit or Fairlight
pages, but they can see the Timeline, and they can make grading changes in the
Fusion or Color pages."*

Two details are worth stealing outright:

* **Propagation is pulled, not pushed, and deliberately so.** *"All changes that
  collaborators make are automatically saved to the project as they're made, so
  no work will ever be lost... However, **each collaborator gets to decide when
  they want to update** the bin, timeline, or clip they're currently working on
  to see the changes made by everyone else, **in order to prevent a kaleidoscope
  of constant alterations to compositions and grades from being a distraction**
  while you're working."* That is a UX decision, not a technical limit, and it is
  the right one for a grading or comp session.
* **When two people must share a timeline, the documented answer is branch and
  merge by hand**: *"this can be accomplished using duplicate timelines and then
  merging the changes back together later on... **Compare With Current
  Timeline**... Right-clicking within the right highlighted area reveals an
  **Accept Change** command."*

Resolve also documents the *non*-collaborative mode, which is the honest baseline
for §C.8's first slice: *"When a shared project is opened by someone else after
it's already been opened, a dialog informs you that it's being opened in
**Read-only mode**."*

**Avid Media Composer** locks at the **bin**, never the sequence:

> *"Your Avid editing application provides a locking mechanism... The method
> allows one user to write to a bin; multiple users can read the files in that
> bin."*
> *"The user who opens the bin first controls the lock... When the person who
> controls the lock closes the bin, it becomes available for another user."*

Because a sequence lives *inside* a bin, locking a bin locks every sequence and
clip in it together — which is why Avid facilities adopt one-bin-per-editor as a
discipline rather than a preference. Two things Avid documents that are unusually
candid and directly relevant:

> *"**Occasionally, when two editors attempt to open a shared bin at the same
> time, both editors get the green lock icon. However, only one editor really has
> the lock**... Both editors can modify their copies of the bin, but only the
> editor that controls the lock... can save that bin. The other editor is warned
> that the bin is locked but is allowed to save a copy."*

> *"**Avoid creating duplicate bins when you modify a locked bin.** If you do
> create a duplicate bin, you should manually merge the changes into the original
> bin and delete the duplicate."*

So the lock is advisory and racy even in the reference implementation of
pessimistic locking, and the recovery path is a manual merge. That is a strong
argument against betting the design on locks being authoritative.

Avid NEXIS workspace permissions are a **separate, storage-layer** mechanism —
the NEXIS administration guide contains no mention of bin locking at all, and
grants combine permissively: *"The permissions set for users, groups, and
Workspaces work together to grant the **highest** level of privilege of the
combination."* Write access to the storage is necessary but not sufficient to
hold a bin lock. Two layers, documented separately — which is exactly the split
§C.7 proposes between recorded roles and advisory claims.

⚠️ **The `.lck` file is `UNVERIFIED`.** Avid documents the mechanism functionally
(a red/green lock button, bold text in the Project window) and **never names a
`.lck` file**. That description is community knowledge, not vendor documentation.

**Adobe Productions** is a genuine gap: `helpx.adobe.com` refused every automated
fetch (timeouts and bot mitigation across several methods), and the previously
cited URL now 404s. **Everything about Productions is `UNVERIFIED` in this
revision** and is deliberately not reproduced from search fragments.

**Perforce Helix Core** is the purest pessimistic model, and its documentation
states the *reason* rather than the mechanism:

> `+l` — *"Exclusive open (locking): If set, only one user at a time can to open
> a file for editing. Useful for binary file types, such as graphics, where
> **merging of changes from multiple authors is meaningless**."*

> *"Some of the main challenges in managing and versioning binaries include:
> **You can't merge them**, making collaboration on an asset difficult."*

That is the whole argument, and it is worth restating because it decides where
locking belongs in *our* design: **optimistic concurrency's entire payoff is
merge. Where merge is impossible, the optimistic bet has no upside and only the
cost of wasted parallel work.** Media files are unmergeable; the *graph* is not.
So locking belongs on media operations (an offload job, a render) and optimistic
concurrency belongs on the graph — which is precisely §C.6's split.

Note also that Perforce separates two things we should keep separate:
`+l` is a **filetype attribute** enforced automatically at open-for-edit;
`p4 lock` is an **explicit command** that blocks *submit* rather than *open*.
And the changelist is atomic: *"either all the files listed in the changelist are
saved in the depot, or none of them are"* — the same guarantee a UCG `Delta`
already provides.

**Git LFS** shows what an advisory lock protocol looks like when it is honest
about being advisory. `git lfs lock` *"Sets the given file path as 'locked'
against the Git LFS server, **with the intention of blocking** attempts by other
users to update the given path"*; `git lfs track --lockable` makes matching paths
*"read-only in the working copy when not locked"*. The server API is small —
`POST /locks`, `GET /locks`, `POST /locks/verify`, `POST /locks/:id/unlock` —
and enforcement happens at push: *"any updated files matching one of 'their'
locks will **halt the push**. At this point, **it is up to the user to resolve the
lock conflict with their team**."*

The project's own design document is the most useful sentence in this whole
section:

> *"We need the ability to lock files to **discourage (we can never prevent)**
> parallel editing of binary files which will result in an unmergeable
> situation."*

And it degrades openly: *"Since File Locking is an early release, and few LFS
servers implement the API, **Git LFS won't halt your push if it cannot verify
locked files**."* An advisory lock that says so is more useful than an
"authoritative" one that races (cf. Avid, above). This is the model for
`collab.claim` in §C.7.

**Figma** is the strongest single primary source here, because they published
their reasoning. On OT:

> *"we didn't want to use operational transforms (a.k.a. OTs)... As a startup we
> value the ability to ship features quickly, and **OTs were unnecessarily complex
> for our problem space**... They're a great way of editing long text documents...
> but they are very complicated and hard to implement correctly. They result in a
> **combinatorial explosion of possible states** which is very difficult to reason
> about... **Since Figma isn't a text editor, we didn't need the power of OTs.**"*

On CRDTs — inspired by, deliberately not one:

> *"**Figma isn't using true CRDTs though.** CRDTs are designed for decentralized
> systems where there is no single central authority... There is some unavoidable
> performance and memory overhead with doing this. Since Figma is centralized
> (our server is the central authority), we can simplify our system by removing
> this extra overhead."*

On the merge rule they actually use — which is §C.6's rule, arrived at
independently:

> *"Figma's multiplayer servers keep track of the latest value that any client has
> sent for a given property on a given object... **This approach is similar to a
> last-writer-wins register in CRDT literature** except we don't need a timestamp
> because the server can define the order of events."*

> *"changes are atomic at the property value boundary... If the text value is B
> and someone changes it to AB at the same time as someone else changes it to BC,
> the end result will be either AB or BC but **never ABC**. That's ok with us
> because Figma is a design tool, not a text editor."*

And on the tree — the passage that matters most for a bin/timeline hierarchy:

> *"Arranging objects in an eventually-consistent tree structure is the most
> complicated part of our multiplayer system... **Two concurrent reparenting
> operations for the same object shouldn't ever result in two copies of that
> object in separate places in the tree.**"*
> *"An example of this is a concurrent edit where one client makes A a child of B
> while another client makes B a child of A. Then A and B are both each other's
> parent, which forms a cycle."*
> *"Figma's multiplayer servers **reject parent property updates that would cause
> a cycle**... But it can still happen on the client... Figma's solution is to
> temporarily parent these objects to each other and **remove them from the tree**
> until the server rejects the client's change."*

Note that Figma's fix **requires a central authority**. Ours cannot assume one
(§C.4's offline constraint), which is what makes the next section's paper the
relevant one.

### C.4a The algorithms, judged against *this* system

**Pessimistic locking.**

* *Fits:* unmergeable, coarse-grained, high-conflict-cost artefacts — a media
  offload job, a render, an export. Perforce states the criterion exactly:
  locking is for things where *"merging of changes from multiple authors is
  meaningless"*.
* *Fails:* the on-set and dailies half of this product is **offline by nature**.
  A design where work stops when a lock server is unreachable cannot serve
  sections A and B. And even the reference implementations are racy and advisory
  (Avid's own "both editors get the green lock icon"; Git LFS's "we can never
  prevent").
* *Verdict:* necessary as a **layer** over media operations, insufficient as
  **the** mechanism for the graph.

**Optimistic concurrency over a causally-ordered op log.**

* *Fits:* an append-only op log is already a commit DAG waiting for parent
  pointers. Conflicts are detectable at attribute granularity because ops name
  `(entity, key)` — the same granularity Figma settled on. Offline work is
  natural: keep committing locally, reconcile later, which is the sneakernet
  reality of a shoot.
* *Costs:* a resolution rule per attribute class, and `prev` must stop being
  load-bearing for inversion (§C.3).
* *Verdict:* **the fit.**

**CRDT** — `automerge` 0.11.0 (MIT, 570 717 downloads, 16 required deps) and
`yrs` 0.27.4 (MIT, 2 897 418 downloads, 10 deps); both actively maintained, both
pure Rust (automerge pins `flate2` to the pure-Rust `zlib-rs` backend). Licences
verified from the repositories' own LICENSE files. So the *dependency* objection
is weaker than assumed — but three substantive ones remain.

1. **A CRDT replaces the op log rather than sitting on it.** Undo, `cause`,
   `depth` and the reactor cascade all have to be rebuilt on top of it.
2. **CRDTs guarantee convergence, not invariants — and Automerge's own
   documentation is careful never to claim otherwise.** It says *"Automerge
   automatically merges the changes together cleanly, so that everybody ends up
   in the same state, and no changes are lost"* — a convergence claim. On
   conflicts: *"Automerge picks one of the concurrently written values as the
   'winner', and it ensures that this winner is the same on all nodes"*, where
   *"'randomly choose' means 'choose one arbitrarily, but in such a way that all
   nodes agree'"*. On sequences it hedges explicitly: the RGA sequence *"means
   that concurrent insertions and deletions can be merged in a manner which
   **attempts to** preserve user intent"*, and *"There are cases where this
   algorithm does not preserve insertion order — primarily when inserting
   elements in reverse."*

   Nothing there prevents two sites converging on a timeline with overlapping
   clips on one track, a take bound to two conflicting materials, a schedule that
   is not a valid packing, or a `Contains` cycle. **Those are our invariants and
   a CRDT will happily converge on a state that violates every one of them.**
3. **Tree moves are a genuinely hard special case, not a consequence of the
   model.** Kleppmann, Mulligan, Gomes and Beresford, *"A highly-available move
   operation for replicated trees"*, state the problem exactly: *"different
   replicas may perform operations that are individually safe, but whose
   combination leads to a cycle... replica 1 moves B to be a child of A, while
   concurrently replica 2 moves A to be a child of B."* Their paper *"demonstrate[s]
   bugs in Google Drive and Dropbox that arise with concurrent moves"*, and notes
   that *"it is not sufficient for the replica that generates a move operation to
   check whether the operation introduces a cycle"*.

   Their solution costs a log replay per move: *"When a replica needs to apply an
   operation with timestamp t, it first **undoes** the effect of any operations
   with a timestamp greater than t, then **performs** the new operation, and
   finally **re-applies** the undone operations."* They are candid about the
   price: *"Ideally, we would want to incur the costs of the undo-do-redo process
   only for genuine move operations."*

   **This is the single most directly applicable paper to CINE-OMNI**, because
   `Tx::reparent` exists and a UCG project *is* a tree of `Contains` edges
   (project → bin → timeline → track → clip). Any design that lets two people
   reorganise concurrently inherits this problem verbatim. The two documented
   answers are Figma's (a server rejects cycles; clients tolerate a transient
   disappearance) and Kleppmann's (undo-do-redo in timestamp order). Ours cannot
   assume Figma's server.

* *Verdict:* **rejected for the graph; retained as a candidate for two specific
  fields** — script dialogue text and ordered list positions — where the problem
  really is sequence merging. And `Tx::reparent` is flagged as needing the
  Kleppmann treatment the day concurrent editing becomes real.

**Operational Transformation.** The correctness conditions are TP1/TP2 (Imine,
Molli, Oster and Rusinowitch state them as C1 and C2):

> *"C1: op1 ∘ T(op2, op1) ≡ op2 ∘ T(op1, op2)"*
> *"C2: T(op3, op1 ∘ T(op2, op1)) = T(op3, op2 ∘ T(op1, op2))"*

The evidence that this is hard is not anecdotal — it is that a theorem prover
broke the published functions:

> *"Proving the correction of these transformation functions is very complex and
> error prone... we have verified the correctness of state-of-art transformation
> functions defined on Strings **with surprising results**. Counter-examples
> provided by the theorem prover have helped us to define new correct
> transformation functions."*

They found a C1 violation in Ellis & Gibbs's original functions and a **C2
violation in Ressel's aDOPTed functions — an algorithm specifically designed to
satisfy C1 and C2**. Their conclusion:

> *"Even on a simple String object, all existing transformation functions are
> incorrect or over-specified... On a simple String object, each time a function
> definition changes, you have to explore **123 different cases** carefully."*

* *Verdict:* **rejected.** OT's value is transforming *sequence* operations;
  this graph's ops are keyed attribute and edge mutations where last-writer-wins
  with causality is simpler and better defined — which is Figma's published
  reasoning for the same choice on the same kind of data. The correctness record
  above settles the rest.

### C.5 The transaction metadata that must be added

Concretely, and in the order of how expensive they are to add later. The op log
file format is the least changeable artefact in the system, so these belong in it
**before** anything ships, even while the product is single-user.

```rust
pub struct DeltaId(pub Uuid);          // NEW — globally unique identity of an op group

pub struct Delta {
    // --- new: identity and causality ---
    pub id: DeltaId,                   // NEW  never reused, never derived from content ordering
    pub parents: SmallVec<[DeltaId; 2]>, // NEW  the frontier this was authored against.
                                       //      One parent = linear; two = a merge commit.
                                       //      This *is* the causality relation; it subsumes
                                       //      vector clocks for this use.
    pub site: SiteId,                  // NEW  the device/session. Distinct from the human.
    pub author: AuthorId,              // RENAME of `actor`, which today conflates the two.
    pub wall_clock_utc: i64,           // NEW  for humans and reports ONLY. Never for ordering.
    pub base_revision: u64,            // NEW  local revision authored against (diagnostics)
    pub content_hash: [u8; 32],        // NEW  H(ops ‖ parents ‖ author ‖ site). Makes the log
                                       //      a Merkle chain: tamper-evident audit for free.
    pub cause: Option<DeltaId>,        // CHANGE from Option<u64>; a revision means nothing remotely

    // --- unchanged ---
    pub revision: u64,                 // keep, but demoted to a site-local index
    pub lamport: u64,
    pub depth: u32,
    pub kind: DeltaKind,
    pub label: String,
    pub ops: Vec<Op>,
}
```

Notes on each choice:

* **`parents`, not a vector clock.** A parent set gives happens-before, merge
  points, and — combined with `content_hash` — a Merkle DAG. One field, three
  properties. Vector clocks would give only the first and grow with the number of
  sites.
* **`site` separate from `author`.** One person on two machines is two sites and
  one author. Locks and presence are per-site; attribution and permissions are
  per-author. Conflating them (as `actor: String` does today) makes both wrong.
* **`wall_clock_utc` is never an ordering key.** Set clocks are unreliable and a
  laptop returning from a location may be hours off. It exists so a production
  report can say "17:42", not so a merge can decide who won.
* **`content_hash` is cheap and unblocking.** It is what turns "we have an op log"
  into "we have an audit trail nobody can quietly rewrite" — the same mechanism
  as the manifest chaining in §B.7, which is an argument for one implementation
  serving both.
* **`Op::SetAttr.prev` must stop being the inversion source across sites.** Two
  options: (a) keep `prev` but treat a delta as invertible only on the site that
  authored it, and implement cross-site undo as a *new forward delta* that
  restores a value (which is what a "revert" is, and it is more honest); or
  (b) drop `prev` from the wire format and reconstruct inverses locally. (a) is
  recommended: it keeps local undo exactly as it is and makes remote undo a
  first-class, auditable operation rather than a silent history rewrite.

### C.6 The recommendation

> **Replicate authored deltas; recompute derived state. Reconcile with
> optimistic concurrency over a causally-parented op log, with advisory
> per-subtree claims for the few genuinely exclusive resources. No CRDT in the
> graph; no OT anywhere.**

The load-bearing move is the first clause, and it is available *because* of an
existing project rule. `CLAUDE.md` requires that "graph writers must be
idempotent: re-running with the same input produces zero ops". If that holds,
then for a given authored state every site's reactors produce the same derived
state. Therefore:

* only `depth == 0`, `kind == Normal` deltas cross the wire;
* each site replays received authored deltas and runs its own cascade;
* derived ops are never merged, because they are never exchanged;
* the merge surface shrinks to authored data, which in this domain is naturally
  partitioned by role — the script supervisor writes takes, the DIT writes
  copies, the editor writes the timeline — so genuine concurrent edits to the
  *same attribute* are rare and can be resolved by rule.

**And it is testable today, single-user.** The convergence test is the whole
foundation:

```
given a project with a full op log,
build a second engine, replay only the depth == 0 Normal deltas,
run the reactor cascade to quiescence,
assert the resulting graph is equal to the original.
```

If that test passes, collaboration is tractable. If it fails, no collaboration
design will save it, and the failure is a bug in reactor idempotence that is
worth finding regardless. This test should be written **now**.

Resolution rules for the authored surface, by attribute class:

| Class | Rule | Rationale |
|---|---|---|
| Scalar attributes (`name`, `status`, `f_stop`) | last-writer-wins by `(lamport, site)` | deterministic, total, cheap |
| Monotonic facts (`verified`, `copy_state`) | **state-machine join**, not LWW — a `failed` never loses to an older `verified` | §B.2's transitions are the merge function |
| Sets (`tags`, edges) | add-wins, with tombstones for explicit removals | removing something you never saw is not an intention |
| Ordered lists (track order, element order) | fractional-index positions, or a sequence CRDT for this field only | LWW on an integer `order` produces duplicates |
| Long text (dialogue) | three-way merge now; sequence CRDT later | the only place OT/CRDT machinery earns its cost |
| Deletion vs edit | **edit-wins**; the delete becomes a proposal a human confirms | losing captured work silently is unacceptable (cf. F1) |

**Rejected alternatives, recorded with reasons:**

| Rejected | Reason |
|---|---|
| Full CRDT graph (`automerge` 0.11.0 MIT / `yrs` 0.27.4 MIT — both healthy, so the dependency objection is *not* the reason) | replaces the op log, undo and cascade; **guarantees convergence but not invariants** — Automerge's own docs claim only that "everybody ends up in the same state", and hedge sequences as *"attempts to preserve user intent"*; tree moves need Kleppmann's undo-do-redo as a special case (§C.4a) |
| Operational Transformation | a theorem prover found a C1 violation in Ellis & Gibbs's original functions **and a C2 violation in Ressel's aDOPTed functions, which were designed to satisfy C1 and C2**; *"123 different cases"* to re-check per change; and Figma rejected it for the same data shape we have (§C.4) |
| Central server / database as the source of truth (the Avid/Resolve shared-project shape) | on-set and DIT work happens with no network; a design that requires reachability cannot serve sections A and B. Note Figma's cycle fix *requires* a central authority — we cannot borrow it |
| Locking as the only mechanism | same reason; and the reference implementations are candid that locks are advisory and racy — Avid: *"both editors get the green lock icon. However, only one editor really has the lock"*; Git LFS: *"discourage (we can never prevent)"* |
| Replicating the full cascade | wasteful and semantically wrong — merges answers instead of recomputing them |
| Wall-clock ordering | set clocks drift and laptops return from location wrong |
| `actor: String` as both person and machine | breaks locks (per-machine) and permissions (per-person) simultaneously |

### C.7 Review, annotation, permissions and offline — where each belongs

The remaining collaboration surface asked about splits cleanly into three
categories, and the split is itself the design.

**Things that are just graph nodes, and therefore need no new machinery.**
Review/approval state, comments, annotations and versions are *authored data
about other authored data*. They replicate, undo, audit and merge by exactly the
mechanisms above, provided they are modelled as nodes rather than as side
tables:

| Concept | Node | Edge | Note |
|---|---|---|---|
| Review / approval | `collab.review` | `References` → the reviewed thing | state is a closed vocabulary; approval of *a specific version* means the review records the `DeltaId` frontier it approved, not just "approved" |
| Comment thread | `collab.comment` | `References` → target; `Contains` → replies | anchoring a comment to a *time range* on a clip needs `range`, in **source** time so it survives a re-edit |
| Drawn annotation | `collab.annotation` | `References` → target | strokes; reuse the `board.stroke` shape rather than inventing a second one |
| Named version / snapshot | `collab.version` | `References` → a `DeltaId` frontier | a "version" is a *label on a point in the op log*, not a copy of the graph |

The last row is the one that saves the most work. Because the op log is
append-only and (with §C.5) parented, **a version is a name for a frontier.**
Restoring one is replaying to that frontier; comparing two is diffing the op
ranges between them. No snapshot duplication, no "save as v3", no divergence
between the version and what it claims to be.

**Things that are a policy layer, not a data layer.** Roles and permissions are
a *check performed before a transaction is accepted*, not a property of the
graph. The honest sequencing (R17) is: record `author` on every delta now, so
the audit trail is meaningful; define roles as data (`collab.role` bound to an
author and a scope) next; enforce last. Enforcement needs a trust boundary to be
meaningful — on a shared filesystem with everyone's own copy of the project,
enforcement is advisory anyway, and pretending otherwise would be dishonest.

**Things that are already solved by the op-log shape.**

* **Offline work and reconnect.** This is not a feature to add; it is the
  default behaviour of a local op log. A site keeps committing while
  disconnected. On reconnect it exchanges the deltas the other side lacks —
  computable from `parents` frontiers, exactly as a DAG sync — and merges by
  §C.6's rules. The on-set reality (no network, media travelling on a shuttle
  drive) means the transport can legitimately be a *file*: an exported delta
  range on the same drive as the rushes. Sneakernet is a first-class transport
  here, not a degraded mode.
* **Branch and merge.** With `parents`, a branch is a frontier that two sites
  extended independently, and a merge is a delta with two parents. That is the
  same structure Git uses, and it is worth being explicit that this is a
  *deliberate* choice of shape: the alternative (a single linear log with
  rebasing) forces op rewriting, and op rewriting is exactly what §C.3 shows is
  unsafe while `prev` is load-bearing.
* **Tamper-evident audit.** `content_hash` over `(ops ‖ parents ‖ author ‖ site)`
  makes the log a Merkle DAG: any retroactive edit changes every descendant
  hash. This is the same construction as the manifest chaining in §B.7 (R18).

### C.8 The minimum honest first slice

Five items. Nothing here is a merge, and nothing here paints a corner.

**1. Extend `Delta` with the §C.5 fields and persist them.** Single-user, they
cost microseconds and change nothing observable. Not doing it means a format
migration later over projects that exist in the world. This is the highest
value-per-line item in the entire collaboration story.

**2. Write the authored-replay convergence test (§C.6).** It is the acceptance
criterion for every later step, it is writable today, and it will find real
reactor bugs immediately.

**3. Single-writer / many-reader over a shared project directory.** One writer
holds an advisory lock file in the project; readers tail `oplog/NNNN.jsonl` and
apply deltas with reactions disabled (`Engine::without_reactions` and
`Engine::replay` already exist). Conflict semantics: none, because there is one
writer. This is genuinely useful — a director or producer watching the cut and
the schedule update live — and it exercises the whole replication path with zero
merge risk.

**4. Advisory claims as ordinary graph nodes.** A `collab.claim` node with
`owner` (author), `site`, `scope` (an entity id whose subtree is claimed),
`expires_at`. Advisory only: it is *shown*, not enforced by a server, so it works
offline and degrades to "here is who else was in this area, and when". Because a
claim is a normal node, it replicates, undoes and audits by the machinery that
already exists — no new subsystem.

**5. The audit trail, which is nearly free.** `author` + `site` + `content_hash`
+ `parents` turn the existing op log into a tamper-evident chain of custody. §B.2
makes every media state transition an op, so the same mechanism answers "who
verified this card and when" and "who changed this take's status to NG". Two
requirements, one implementation.

**Explicitly not in the first slice:** merge, CRDTs, live cursors, a server,
presence, real-time conflict UI, permissions enforcement. Roles and permissions
should be *recorded* (so the audit trail is meaningful) long before they are
*enforced*.

---

## D. Proposed UCG schema

Fitting `crates/ucg-schema/src/kinds.rs` conventions (`module.type` strings) and
`attrs` (short snake_case). Builds on
[`09`](09-editorial-media-model.md) §8's `media.*` proposal; where that document
proposes `media.material` / `media.representation`, this one uses those names and
notes the currently-implemented `asset.media` as the interim carrier.

### D.1 Node kinds

| Kind | New? | Represents | Deterministic id | Key attributes |
|---|---|---|---|---|
| `project` | exists | the film | authored | `name`, `production_number`, `playback_role` |
| `script.document` | exists | the screenplay | exists | — |
| `script.section` | exists | **act / sequence** — reuse, do not invent | exists | `name`, `order` |
| `script.scene` | exists | the scene | exists | `scene_number`, `heading` |
| `logistics.shot` | exists, **id must change** | a setup / slate | `derived(scene, "shot:<slate>")` — **not** the previs camera (F1) | `slate`, `size`, `movement`, `description`, `captured` |
| `logistics.take` | exists, **id must change** | one performance | authored id, or `from_key("take:<reel>:<start-tc>")` on ingest — **never** the take number (§A.3) | `take_number`, `sound_take_number`, `status`, `circled`, `printed`, `mos`, `start_tc`, `end_tc`, `duration`, `notes`, `sup_notes`, `series` |
| `production.record` | **new** | one device's recording of one take (§A.4) | `derived(take, "rec:<device>:<clip-name>")` | `record_role` (`camera`\|`sound`\|`video_assist`\|`stills`\|`data`), `device_label` (`A`, `B`, `SND`), `clip_name`, `reel`, `start_tc`, `rate`, `sync_method`, `sync_offset`, `sync_confidence`, `channel_roles`, `lens_serial`, `filters` |
| `production.lens` | **new** | a physical lens | `from_key("lens:<make>:<model>:<serial>")` | `make`, `model`, `serial`, `focal_range_mm`, `distortion_profile` |
| `media.source` | proposed in 09 | card / reel / roll / magazine | `from_key("source:<card key>")` | `reel`, `card_id`, `camera`, `shoot_date`, `medium` |
| `media.material` | proposed in 09 | the content (interim: `asset.media`) | see 09 §8.4 | as 09 |
| `media.representation` | proposed in 09 | one encoding (interim: the media node itself) | see 09 §8.2 | as 09 |
| `media.stream` | exists | one elementary stream | `derived(media, "stream:<i>")` | `codec`, `decodable`, `order` |
| `media.derivative` | exists | proxy / thumbnail / waveform / render | `from_key("media:derivative:<kind>:<src hash>:<recipe>")` | `derivative_kind`, `recipe`, `media_path` |
| **`media.volume`** | **new** | a storage destination (a drive, a NAS share, an LTO tape) | `from_key("volume:<uuid or label>")` | `label`, `volume_kind` (`card`\|`shuttle`\|`nas`\|`lto`\|`cloud`), `location` (`onset`\|`post`\|`offsite`), `capacity_bytes` |
| **`media.copy`** | **new** | **one copy of one representation on one volume — the unit of verification** | `derived(representation, "copy:<volume id>")` | `copy_state` (**`original`\|`verified`\|`failed`** — the ASC MHL vocabulary verbatim, plus the internal-only `discovered`\|`copying`\|`copied`\|`verifying` which are never exported), `rel_path`, `size`, `alg`, `src_digest`, `dst_digest`, `verified_at`, `verifier`, `failure_reason`, `previous_path` |
| **`media.manifest`** | **new** | one transfer's manifest / generation (an ASC MHL generation, when exported) | `derived(volume, "manifest:<generation>")` | `generation` (1-based, dense — ASC MHL §7.4.1), `created_at`, `creator`, `tool`, `entry_count`, `format` (`asc-mhl-v2` \| `mhl-v1.1` \| native), `process` (`in-place`\|`transfer`\|`flatten`), `c4` (self-reference digest) |
| **`production.camera_report`** | **new** | derived per camera per day | `derived(day, "camrep:<device>")` | derived content |
| **`production.sound_report`** | **new** | derived per sound roll per day | `derived(day, "sndrep:<roll>")` | derived content |
| **`production.daily_report`** | **new** | derived per shoot day | `derived(day, "dpr")` | derived content |
| **`collab.claim`** | **new** | an advisory claim on a subtree (§C.8) | authored | `owner`, `site`, `scope`, `claimed_at`, `expires_at` |
| **`collab.review`** | **new** | a review/approval record | `derived(target, "review:<n>")` | `reviewer`, `state` (`pending`\|`approved`\|`rejected`), `at`, `approved_frontier` |
| **`collab.comment`** | **new** | a comment, optionally anchored to a source-time range | authored | `author`, `at`, `text`, `range` (source time), `resolved` |
| **`collab.annotation`** | **new** | a drawn annotation on a frame | authored | `author`, `at`, `frame`, strokes (reuse `board.stroke` children) |
| **`collab.version`** | **new** | a **named point in the op log**, not a copy of the graph | authored | `name`, `frontier` (list of `DeltaId`), `at`, `author` |
| **`collab.role`** | **new** | an author's role within a scope (recorded before it is enforced, R17) | `derived(author, "role:<scope>")` | `author`, `scope`, `role` |

### D.2 Edge kinds — no new `EdgeKind` variant required

The six existing kinds suffice, which is a good sign. The choice of kind per
relation is the design, so each is justified:

| From → To | Edge | Why this kind, and not another |
|---|---|---|
| `project` → `script.document` | `Contains` | ownership and lifetime |
| `script.document` → `script.section` → `script.scene` | `Contains` | existing hierarchy |
| `logistics.shot_list` → `script.scene` | `DerivedFrom` | existing; a scene change must re-evaluate the shot list |
| `logistics.shot_list` → `logistics.shot` | `Contains` | existing — **but see F1**: the shot must become undeletable once it has takes |
| `logistics.shot` → `previs.camera` | `DependsOn` | existing; the shot is *informed by* previs, it is not *identified by* it |
| `logistics.shot` → `logistics.take` | `Contains` | a take belongs to exactly one shot and dies with it — legitimate ownership, made safe by the no-delete rule |
| `logistics.take` → `production.record` | `Contains` | a record has no meaning without its take |
| `logistics.take` → `logistics.take` (pickup) | `References` | must **not** propagate: annotating the original must not dirty the pickup |
| `production.record` → `media.material` | **`Binds`** | **the load-bearing edge.** Media changing means *relink*, never *recompute*. `Binds::propagates()` is `false`, which is exactly right: replacing a file must not dirty captured take metadata |
| `production.record` → `media.stream` | `References` | per-record channel/angle selection where needed |
| `production.record` → `production.lens` | `References` | a lens is shared across many records and outlives them |
| `media.material` → `media.source` | `DerivedFrom` | provenance *and* propagation: correcting a reel name on the card should re-evaluate conform on everything shot on it (09 §8.3) |
| `media.material` → `media.representation` | `Contains` | a representation cannot outlive its material |
| `media.representation` → `media.stream` | `Contains` | same |
| `media.derivative` → `media.material` | `DerivedFrom` | **the invalidation edge.** A *system-generated* proxy gets it; a *user-supplied* proxy must not (§B.8) |
| **`media.copy` → `media.representation`** | **`DerivedFrom`** | a copy is genuinely derived from the thing it copies, and if the source is replaced every copy of it is stale. Propagation is wanted here |
| **`media.copy` → `media.volume`** | **`References`** | the volume outlives the copy and is shared; a volume going offline must not dirty every copy on it |
| **`media.manifest` → `media.copy`** | **`References`** | a manifest names copies it does not own |
| **`media.manifest` → `media.manifest`** (generation N → N−1) | **`DerivedFrom`** | the chain of custody: generation N *is* derived from and asserts N−1 |
| `production.camera_report` → `logistics.take` | `DerivedFrom` | reports are recomputed from takes and cannot drift |
| `nle.clip` → `media.material` | `Binds` | existing; the edit references the material, never the file (09 §8.5) |
| `nle.clip` → `script.scene` | `SyncedTo` | existing, unchanged |
| **`collab.claim` → (claimed subtree root)** | **`References`** | a claim must not own or dirty what it claims |
| **`collab.review` → (reviewed thing)** | **`References`** | same |

**The traceability answer (decision question 1).** A take is traceable to its
exact source media, and back, by pure graph walk with no duplicated pointers:

```
project ─Contains→ script.document ─Contains→ script.section ─Contains→ script.scene
                                                                          ▲ DerivedFrom
                                              logistics.shot_list ────────┘
                                                    │ Contains
                                              logistics.shot
                                                    │ Contains
                                              logistics.take
                                                    │ Contains
                                              production.record ──Binds──→ media.material
                                                    │                          │ Contains
                                                    │                     media.representation
                                                    │                          │ Contains
                                                    │                     media.stream
                                                    └──References──→ media.source (card)

                        media.copy ──DerivedFrom──→ media.representation
                             └──References──→ media.volume
```

and editorial reaches the take from the cut without a second edge:
`nle.clip --Binds--> media.material <--Binds-- production.record <--Contains-- logistics.take`.

### D.3 New attribute keys for `ucg_schema::attrs`

```
// take / production
TAKE_NUMBER, SOUND_TAKE_NUMBER, SLATE, TAKE_STATUS, CIRCLED, PRINTED, MOS,
START_TC, END_TC, DURATION, SUP_NOTES, SERIES, PICKUP_OF,
RECORD_ROLE, DEVICE_LABEL, CLIP_NAME, REEL, SYNC_METHOD, SYNC_OFFSET,
SYNC_CONFIDENCE, CHANNEL_ROLES, LENS_SERIAL, FILTERS, CAPTURED,
MATCH_KEY, MATCH_RANK,          // §A.7 — how this record was attached to its take

// dailies
COPY_STATE, VOLUME_KIND, VOLUME_LOCATION, REL_PATH, DIGEST_ALG,
SRC_DIGEST, DST_DIGEST, VERIFIED_AT, VERIFIER, FAILURE_REASON, GENERATION,
PREVIOUS_PATH,                  // ASC MHL rename chain (§B.7.5)
CONTENT_HASH, STRUCTURE_HASH,   // the two directory digests (§B.7.4)

// collaboration
OWNER, SITE, SCOPE, CLAIMED_AT, EXPIRES_AT, REVIEW_STATE, REVIEWER,
APPROVED_FRONTIER, AUTHOR, FRONTIER, ROLE, RESOLVED,
```

Two typed components, following the existing `Component` pattern:

```rust
pub struct TakeStatus { … }        // KEY = attrs::TAKE_STATUS   — closed vocabulary (§A.2)
pub struct CopyVerification { … }  // KEY = attrs::COPY_STATE    — the §B.3 premises, together
```

`CopyVerification` holding all of the premises in **one** component is deliberate:
it makes it impossible to write `verified_at` without also writing the digests
that justify it, because they are one `Value` tree set by one `tx.set_as`.

---

## E. Recommendations, with rejected alternatives

| # | Recommendation | Rejected alternative | Why |
|---|---|---|---|
| R1 | Take/shot ids minted once and never derived from a mutable label | `derived(shot, "take:<n>")` as today | renumbering destroys the node and orphans its media (§A.3) |
| R2 | `production.record` as the take↔media intermediary | N `Binds` edges from take to media | `ucg_core::Edge` has no attributes; role, reel, timecode offset and sync state have nowhere to live (§A.4) |
| R3 | Verification is per **copy on a volume**, not per media | a `verified` flag on the media node | "safe to format the card" is a predicate over the *set* of copies (3-2-1); one flag cannot express it (§B.1) |
| R4 | `verified` reachable only from `verifying`, only with the ten premises of §B.3, enforced by an unconstructible `Verified` type | a boolean attribute anyone can set | the directive's hard requirement; a boolean is exactly how an incomplete copy masquerades as verified media |
| R5 | Destination digest computed by **re-reading the file after fsync** | hashing the write buffer | hashing the buffer verifies RAM, not storage |
| R6 | Temp-name + fsync + fsync(dir) + rename | writing directly to the final path | otherwise a crash leaves a short file at a plausible path |
| R7 | XXH128 as the manifest default (matching the ASC MHL reference tool), XXH64 for older-tool interop | SHA-256 for everything | 55× measured on the mandatory verification pass (§B.4), and SHA-256 is not even in the ASC MHL vocabulary (§B.7.6); slow enough that people skip verification |
| R8 | BLAKE3 as the identity migration target; SHA-256 retained meanwhile | staying on SHA-256 indefinitely | 14× measured even in BLAKE3's hobbled pure-Rust build, and still cryptographic; but the C/asm build fails on this toolchain, and neither digest is expressible in ASC MHL — so this is an *internal identity* migration, not a manifest change |
| R9 | MD5/SHA-1 **read-only**, for foreign manifests | generating MD5 as the primary digest | slowest of the fast options, broken for collisions — worst of both |
| R10 | Namespace the media identity key by algorithm now | `"media:file:<hex>"` | a later algorithm change silently mints different entities for the same file (F4) |
| R11 | No reactor may delete a node with captured descendants | the current unconditional stale-shot delete | reproduced: a script/previs edit deletes takes (F1) |
| R12 | Replicate `depth == 0` deltas; recompute derived state | replicating the full cascade | merges answers instead of recomputing them; wasteful and semantically wrong (§C.6) |
| R13 | Optimistic concurrency over a causally-parented op log + advisory claims | full CRDT graph; OT; central server; locking alone | see the table in §C.6 and the evidence in §C.4/§C.4a |
| R24 | Locking belongs on **media operations** (offload, render, export); optimistic concurrency belongs on **the graph** | one mechanism for both | Perforce states the criterion: locking is for artefacts where *"merging of changes from multiple authors is meaningless"*. Media files are unmergeable; the graph is not (§C.4) |
| R25 | Partition concurrent access **by discipline**, not by region — editor/colourist/comp touch disjoint attribute sets on the same clip | one lock per clip | this is Resolve's documented three-tier model and it is why three people can hold the same clip without conflict (§C.4) |
| R26 | Remote changes are **pulled**, not pushed into a live session | auto-propagating every remote delta | Resolve does this deliberately — *"to prevent a kaleidoscope of constant alterations... from being a distraction"* — and it is a UX requirement, not a technical limitation (§C.4) |
| R27 | Treat `Tx::reparent` as the known-hard case; adopt Kleppmann's undo-do-redo *if and when* concurrent reorganisation ships | assuming tree moves fall out of the merge rules | concurrent moves produce cycles or duplicates, and *"it is not sufficient for the replica that generates a move operation to check whether the operation introduces a cycle"* (§C.4a) |
| R28 | The take-status model is a **flag set** (iXML `TAKE_TYPE`) plus an independent `circled` boolean | a single-valued status enum | `<TAKE_TYPE>FALSE_START,PICKUP</TAKE_TYPE>` is legal; an enum cannot round-trip a real iXML file (§A.2, §A.8.1) |
| R29 | Model lens metadata on the Cooke /i field set; treat SMPTE RDD 18 as a lossy projection | deriving the model from the SMPTE standard | RDD 18 has no entrance pupil, serial, hyperfocal, near/far, HFOV, distortion or shading, and caps `Lens Attributes` under 64 bytes (§A.8.5) |
| R30 | Do not model slates on MXF DMS-1; export target at most | using ST 380 as the take schema | `Take Number` is a **Uint16** (cannot express "3A"), and scene and take live in *different frameworks* so the pair is not one record (§A.8.7) |
| R31 | Store timecode user bits as four raw bytes plus a BGF-derived interpretation hint | parsing them as a date | vendors disagree on day/month order and one exposes both; ST 12-1 permits *"any manner without restriction"* (§A.8.6) |
| R32 | Record `jammed_at` and the generator's rated ppm on each `production.record` | treating timecode sync as binary | drift is computable — ARRI's jam holds *"less than one frame for each eight hours"*, generators are rated 0.1–0.5 ppm — so §A.7 rank 2 can decide whether a match is trustworthy (§A.8.6) |
| R33 | Make the identity-digest algorithm a **capability-gated** choice, not a constant | hardcoding SHA-256 or BLAKE3 | SHA-256 throughput varies ~3.6× on the presence of Intel SHA extensions; the same shape as `probe::is_decodable` (§B.4) |
| R34 | Carry `circled` as a first-class attribute we own, not something we expect to import | relying on interchange to bring it | it has **no standard carrier in ALE** — verified against a real 129-column Avid export (§A.8.3) |
| R14 | Add `id`, `parents`, `site`, `author`, `wall_clock_utc`, `content_hash` to `Delta` **now** | adding them when collaboration ships | the op-log file format is the least changeable artefact in the system (§C.5) |
| R15 | Cross-site undo is a new forward "revert" delta, not an inverse replay | replaying `Op::inverse` across sites | `prev` is only valid against the state it was generated on; remote inverse replay silently rewrites history (§C.3) |
| R16 | Write the authored-replay convergence test today | waiting for a collaboration project | it is the acceptance criterion for everything later and finds reactor-idempotence bugs now (§C.6) |
| R17 | Record roles/permissions before enforcing them | building permission enforcement first | the audit trail needs authorship immediately; enforcement is a product decision that can wait |
| R18 | The op log's Merkle DAG is the chain of custody; ASC MHL is the interchange format read and written at the boundary | relying on the ASC MHL chain as the audit trail | the chain has **no inter-generation hash link and no signature**, and `ascmhl_chain.xml` is itself unhashed — an unprotected trust root (§B.7.5) |
| R19 | Read ASC MHL v2 **and** MHL v1.1; write only ASC MHL v2 | writing v1 for compatibility | v1 has no `action`, so writing it records less than we know (§B.7.8) |
| R20 | Default manifest digest **XXH128**, matching `ascmhl_default_hashformat`; implement C4 (SHA-512 + base58, left-padded with `1` to 88) | choosing our own default | XXH128 is the reference tool's default; C4 is *mandatory* in `hashlistreference` and the chain (§B.7.6) |
| R21 | Map `copy_state` onto `original \| verified \| failed` exactly | a richer internal vocabulary written into manifests | the standard's enumeration is closed; a fourth value breaks the round trip (§B.7.3) |
| R22 | Adopt the "new algorithm only in a verifying pass" rule for the R8 digest migration | re-hashing files with BLAKE3 and marking them verified | it would launder unverified files into verified ones (§B.3 rule 10) |
| R23 | Emit both `content` and `structure` directory hashes; mirror `ASCMHL.xsd` locally; do not vendor the spec PDFs | one directory digest; relying on the schema's network import; committing the PDFs | they answer different questions (§B.7.4); `ASCMHLDirectory.xsd` imports over the network; the PDFs carry no licence (§B.7.1) |

---

## F. What this document does not claim

* **Section A's format field names *are* now verified** (§A.8) — iXML, BWF
  `bext`, ALE, ASC CDL, ACES AMF, Cooke /i, ZEISS, ST 12-1, ST 380 and RDD 18
  were all fetched. What is *not* verified is some of the surrounding workflow
  folklore: the spoken slate call-out alphabet, the lined-script squiggle
  convention, and film-production definitions of "false start" and "series
  takes". Those are marked inline and in §G.
* **Adobe Productions is a genuine gap.** `helpx.adobe.com` refused every
  automated fetch method. Nothing about it is asserted.
* **Section B's ASC MHL chapter *is* verified** — spec, guidelines, XSDs,
  reference implementation and generated examples were all fetched and read
  (§B.7). Two caveats are recorded there rather than smoothed over: the
  specification's Appendix D contradicts its own examples on encoding, and
  several element orderings differ between the spec prose and the shipped XSD.
  Where they disagree, this document follows the shipped XSD and the examples,
  and says so.
* **No vendor internals are asserted.** Resolve, Avid, Perforce, Git LFS and
  Figma are quoted only on *documented behaviour* and, in Figma's case, on
  reasoning they published themselves. Silverstack, YoYotta and Hedge appear (if
  at all) as workflow concepts; none of their designs is reproduced.
* **The benchmark figures are from one machine with no SHA hardware
  acceleration**, run twice (loaded and quiet). The *ordering* is robust across
  both; the absolute SHA-256 number is not portable and is flagged as such.
* **BLAKE3's assembly-path throughput was not measured here** — the C build
  failed under this machine's `zig cc` shim.

---

## G. UNVERIFIED register

| # | Claim | Status | What would settle it |
|---|---|---|---|
| U1 | ASC MHL XML structure, `action` vocabulary, chain/generation mechanism, hash vocabulary | **RESOLVED — verified** (§B.7) | spec v1.0 + Implementation Guidelines + XSDs + reference impl + generated examples, all fetched |
| U2 | ASC MHL specification and reference-tool licences | **RESOLVED** (§B.7.1): tool/XSDs **MIT**; **the specification PDFs carry no licence and no copyright notice at all** | — |
| U3 | Original (pre-ASC) MHL element names and documentation | **RESOLVED — verified** (§B.7.8) | v1.1 XSD from mediahashlist.org + real `.mhl` files from `pomfort/mhl-tool` |
| U4 | C4 ID: digest, encoding, length, standardisation, implementations | **RESOLVED — verified** (§B.7.7): SMPTE ST 2114:2017, SHA-512, base58, 90 chars, **no IEEE number**; Avalanche-io `c4` is Apache-2.0 | — |
| U4a | IEEE Xplore's own metadata for ST 2114 | **UNVERIFIED** — the Xplore page returned empty content; the *absence* of an IEEE designation inside the standard text is verified | fetch the Xplore record |
| U4b | The `cccc.io` list of C4 implementations and their licences (beyond Avalanche-io `c4`, which was read) | **LIGHTLY VERIFIED** — page is JS-rendered | fetch each repo's LICENSE |
| U5 | iXML element names and openness | **RESOLVED — verified** (§A.8.1): revision 3.01; *"the iXML specification is released to the public domain"* (prose © Gallery UK); real element names quoted | — |
| U6 | BWF `bext` field list and the EBU document number | **RESOLVED — verified** (§A.8.2): EBU **Tech 3285 v2.0**, May 2011; full struct and byte offsets | — |
| U7 | ALE header structure and column names | **RESOLVED — verified** (§A.8.3), including the negative: **`Circled` is not a standard ALE column** | — |
| U8 | Cooke /i and ZEISS eXtended Data payloads; openness | **PARTIALLY RESOLVED** (§A.8.5): /i field list quoted from Cooke; ZEISS field list quoted from ZEISS; **/i EXTENDED commands are by-request/NDA and no ZEISS format spec exists publicly** | a partner NDA |
| U9 | ASC CDL formats | **RESOLVED, with an asymmetry** (§A.8.4): the **specification is not publicly downloadable** (`theasc.com` 403s; obtained by e-mail request, no licence stated) but the **official XSD is public** in the Apache-2.0 AMF repo | — |
| U10 | ACES AMF contents and openness | **RESOLVED — verified** (§A.8.4): schema Apache-2.0, prose CC BY 4.0, `urn:ampas:aces:amf:v2.0` | — |
| U11 | Timecode generator drift figures | **RESOLVED — verified** (§A.8.6): vendor-stated 0.1–0.5 ppm; ST 12-1's own ±100 ppm floor; ARRI's *"less than one frame for each eight hours"* | — |
| U12 | Scene-numbering conventions | **RESOLVED for the written convention** (§A.8.8): Elkins gives the excluded letters **I, O, Q, S, Z** and the reason, plus the UK/US split. **The spoken "24 apple" call-out alphabet remains `UNVERIFIED`** — no canonical list, and practitioner lists conflict with Elkins | a production-management publication |
| U12a | The lined-script squiggle: "off-camera" vs "face visible" | **UNVERIFIED — two fetched sources disagree** and no guild-published normative definition exists (§A.8.8) | a guild publication |
| U12b | `false_start` and `series` as *film-production* definitions; `pickup` in the post-principal-photography sense | **UNVERIFIED** — the two standard references could not be fetched. iXML's `TAKE_TYPE` at least confirms `FALSE_START` and `PICKUP` are standard vocabulary (§A.8.8) | Miller's glossary; Cybulski, *Beyond Continuity* |
| U13 | 3-2-1 rule origin | **RESOLVED — verified** (§B.1): CISA-hosted US-CERT/CMU document, whose **own footnote credits Krogh, *The DAM Book* 2nd ed., p. 207**. It is **not** "ST19-001". The 2005 first-edition claim remains `UNVERIFIED` | the 1st edition |
| U14 | Camera/sound take-numbering divergence | **INDIRECTLY SUPPORTED** (§A.8.1, §A.6): iXML types `<TAKE>` as a *string* admitting `WT1`, and sound and camera reports are keyed by different rolls — but no source states the divergence as a rule | a production-sound publication |
| U15 | Resolve collaboration granularity and conflict rules | **RESOLVED — verified** (§C.4): three tiers (bin / clip / timeline), *"first come, first served"*, pull-based refresh, duplicate-and-merge for shared timelines | — |
| U16 | Avid bin locking granularity and conflict rule | **RESOLVED — verified** (§C.4): bin-level, first-opener wins, and Avid's own admission that the lock can race | — |
| U16a | Avid `.lck` files | **UNVERIFIED** — Avid documents the mechanism functionally and never names a `.lck` file; the description is community knowledge (§C.4) | Avid documentation |
| U17 | Adobe Productions locking behaviour | **UNVERIFIED — a genuine gap.** `helpx.adobe.com` refused every automated fetch and the previously cited URL 404s. Deliberately not reconstructed from search fragments | a browser session on the four pages named in §C.4 |
| U18 | Perforce `+l`; Git LFS locking API | **RESOLVED — verified** (§C.4): `+l` exclusive open vs `p4 lock` on submit; the four LFS endpoints; and LFS's own *"we can never prevent"* | — |
| U19 | Figma's account of CRDT vs OT | **RESOLVED — verified** (§C.4): OT *"unnecessarily complex for our problem space"*; *"Figma isn't using true CRDTs"*; LWW at the property boundary; the cycle problem and their transient-removal fix | — |
| U20 | CRDT literature | **RESOLVED — verified**, with a correction (§C.4a): **RR-7506 does not define Strong Eventual Consistency** — the companion SSS 2011 paper does. Kleppmann et al.'s move operation fetched and quoted | — |
| U21 | OT correctness literature | **RESOLVED — verified** (§C.4a): C1/C2 stated; a theorem prover broke both Ellis & Gibbs's and Ressel's functions; *"123 different cases"* | — |
| U21a | Ressel et al. CSCW'96; Imine et al. TCS 2006; Sun & Ellis CSCW'98 | **UNVERIFIED** — paywalled or OCR-corrupted; the ECSCW 2003 and TOCHI 1998 papers were used instead | institutional access |
| U22 | `automerge` and `yrs` reality | **RESOLVED — verified**: `automerge` 0.11.0, **MIT**, 570 717 downloads, 16 required deps, pure Rust (`flate2` pinned to `zlib-rs`); `yrs` 0.27.4, **MIT**, 2 897 418 downloads, 10 deps. Both actively maintained | — |
| U23 | C2PA and `c2pa-rs` | **RESOLVED — verified**: spec is at **v2.4**, published by the **Joint Development Foundation**, licensed **CC BY 4.0**; `c2pa` 0.90.20 is **MIT OR Apache-2.0** but pulls **83 required runtime dependencies** — a large audit surface for a provenance library. Content Credentials is the consumer brand, not a synonym for C2PA | — |
| U23a | How the Content Authenticity Initiative relates to C2PA | **UNVERIFIED** — no quotable sentence found on either site | — |
| U24 | xxHash / BLAKE3 published benchmarks | **RESOLVED — verified** (§B.4): xxHash's README table (i7-9700K, clang 10, `-O3`) and BLAKE3's chart data (AWS c5.metal, Cascade Lake-SP 8275CL, single thread) | — |
| U24a | SHA-256 throughput *with* Intel SHA extensions | **UNVERIFIED** — no SHA-NI CPU available. Intel's own kernel patch reports *"up to 3.6x speedup over existing SSSE3 enhanced routines"*, which is what §B.4 cites instead | a SHA-NI machine |
| U25 | Published hash test vectors quoted verbatim | **PARTIALLY RESOLVED**: MD5 and SHA-1 cryptanalysis now cited to the primary IACR papers (§B.4). The "Reference" column of the test-vector table remains agreement-between-implementations rather than a fetched citation | fetch the reference test-vector files |
| U26 | `quick-xml`, `bs58`, `memmap2`, `walkdir`, `twox-hash` | **RESOLVED — verified**: `quick-xml` 0.42.0 **MIT** (one dep, memchr); `bs58` 0.5.1 `MIT/Apache-2.0`, zero deps, **frozen since 2024-03-19**; `memmap2` 0.9.11 `MIT OR Apache-2.0`; `walkdir` 2.5.0 Unlicense/MIT, **last commit 2024-12-31 — finished, not actively maintained**; `twox-hash` 2.1.4 **MIT**, zero deps | — |
| U26a | `bs58`'s MIT file names the wrong project | **VERIFIED ODDITY** — its `LICENSE-MIT` reads *"Copyright (c) 2016 The roaring-rs developers"*, a copy-paste artefact worth knowing before a provenance audit | — |

### G.1 Rust crate reality — verified on crates.io and against repository LICENSE files

Fetched 2026-09-08. Licences confirmed twice: the crates.io metadata field *and*
the actual LICENSE file in each repository.

| Crate | Version | Licence | Total dl | Last release | Deps | Pure Rust | Note |
|---|---|---|---|---|---|---|---|
| `xxhash-rust` | 0.8.18 | **BSL-1.0** | 100.0 M | 2026-07-21 | **0** | yes | no default features, unconditional `no_std`, `const fn` variants; **the only non-MIT/Apache crate proposed here** |
| `twox-hash` | 2.1.4 | MIT | 236.6 M | 2026-08-27 | **0** | yes | more downloads and more recent releases; **but its defaults pull `rand` and are not `no_std`** — needs `default-features = false` |
| `blake3` | 1.8.7 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | — | — | — | with `pure` | default build compiles C/asm and **fails under this machine's `zig cc`** |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | — | — | — | yes | already in use |
| `md-5` | 0.10.6 | MIT OR Apache-2.0 | — | — | — | yes | read-only interop |
| `sha1` | 0.10.7 | MIT OR Apache-2.0 | — | — | — | yes | read-only interop |
| `crc32fast` | 1.5.1 | MIT OR Apache-2.0 | — | — | — | yes | error detection only |
| `quick-xml` | 0.42.0 | MIT | 403.0 M | 2026-08-22 | **1** (memchr) | yes | for ASC MHL; licence file is `LICENSE-MIT.md`, not `LICENSE` |
| `bs58` | 0.5.1 | `MIT/Apache-2.0` | 127.0 M | 2024-03-19 | **0** | yes | for C4. **Frozen since 2024**; and its MIT file names *"The roaring-rs developers"* — a copy-paste artefact |
| `memmap2` | 0.9.11 | MIT OR Apache-2.0 | 333.5 M | 2026-06-22 | 1 (libc, FFI decls only) | yes | optional, for large-file hashing |
| `walkdir` | 2.5.0 | Unlicense/MIT | 608.2 M | 2024-03-01 | 2 | yes | **finished, not actively maintained** (last commit 2024-12-31) |
| `automerge` | 0.11.0 | **MIT** | 570.7 k | 2026-08-12 | 16 | yes (`flate2` pinned to `zlib-rs`) | **rejected for the graph** (§C.4a) — on invariants, not on health |
| `yrs` | 0.27.4 | **MIT** | 2.9 M | 2026-08-22 | 10 | yes | same |
| `c2pa` | 0.90.20 | MIT OR Apache-2.0 | 9.9 M | 2026-09-04 | **83** | yes by default (`openssl` is optional) | provenance; 83 required deps is a large audit surface |

All of these pass this project's `deny.toml` allowlist, which already permits
`BSL-1.0`, `CC0-1.0`, `MIT`, `Apache-2.0` and `Unlicense`.

**Recommendation on the xxHash crate.** `xxhash-rust` with
`features = ["xxh3"]` gives XXH3/XXH128, **zero dependencies**, unconditional
`no_std` and `const fn` — at the cost of adding **BSL-1.0**, a third licence
family, to the bill of materials. `twox-hash` is the more-downloaded crate but
its default features pull `rand` and forfeit `no_std`, so it must be taken as
`default-features = false, features = ["xxhash3_128"]`. Both implement all four
variants; the frequently-repeated claim that only one supports XXH3 is false.
**Prefer `xxhash-rust`** — zero dependencies on a hot path in a media tool is
worth one extra permissive licence, and `deny.toml` already allows it.

**Verified in this revision (not in the register):**

| Fact | How |
|---|---|
| `xxhash-rust` 0.8.18 is `BSL-1.0` (Boost) | `license` field in the downloaded crate's `Cargo.toml`, plus its `LICENSE` file which opens "Boost Software License - Version 1.0 - August 17th, 2003" |
| `blake3` 1.8.7 is `CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception` | downloaded crate `Cargo.toml`; three LICENSE files present |
| `md-5` 0.10.6, `sha1` 0.10.7, `sha2` 0.10.9, `crc32fast` 1.5.1 are `MIT OR Apache-2.0` | downloaded crates' `Cargo.toml` and LICENSE files |
| All of the above pass this project's own licence policy | `deny.toml` `[licenses].allow` includes `BSL-1.0`, `CC0-1.0`, `Apache-2.0`, `MIT` |
| `blake3`'s default build fails on this toolchain | captured `cc-rs` error compiling `c/blake3_sse2_x86-64_unix.S` via `zigcc` |
| `xxhash-rust` matches reference xxHash values | test vectors computed and compared (§B.4) |
| F1 (reactor deletes takes) | **executed** against the working tree: 3 takes deleted, 4 `DeleteNode` ops (§B.6) |
| F2 (`Fast` verification passes corruption) | **executed**: 1 bit flipped at 2 MiB in a 4 MiB file passes `Fast::verify()` (§B.5) |
| `Fast` *does* catch truncation (length is hashed) | executed (§B.5) |
| `EdgeKind::Binds` and `References` do not propagate dirtiness; `DependsOn`, `DerivedFrom`, `SyncedTo` do | `EdgeKind::propagates()`, `crates/ucg-core/src/node.rs` |
| `Delta` carries `actor`, `lamport`, `cause: Option<u64>`, `depth`, `kind`, and no delta id / parents / site / content hash | `crates/ucg-core/src/tx.rs` |
| Hash throughput figures | measured twice (loaded and quiet), §B.4 |
| ASC MHL: root `<hashlist version="2.0" xmlns="urn:ASC:MHL:v2.0">`; `action` enum `original\|verified\|failed`; hash vocabulary `md5, sha1, c4, xxh64, xxh3, xxh128`; `ascmhl_default_hashformat = "xxh128"`; chain file `ascmhl_chain.xml` in namespace `urn:ASC:MHL:DIRECTORY:v2.0` | spec v1.0 PDF, `xsd/ASCMHL.xsd`, `ascmhl/__version__.py`, generated example scenarios — all fetched |
| ASC MHL §5.6.4: "Only hashes labeled as either `original` or `verified` can be used for verification. Hashes labeled as `failed` cannot be used." | quoted from the specification |
| ASC MHL tool + XSDs are MIT; the specification PDFs carry **no licence and no copyright notice** | `LICENSE.md`, `setup.py`, module headers; and a zero-hit grep for copyright/licence across both PDFs' extracted text |
| ASC MHL chain has no inter-generation hash link and no signature (MHL v1.1 had optional XML-DSig; v2 dropped it) | v2 XSDs vs the v1.1 XSD's `<xs2:Signature>` element |
| C4 = SMPTE ST 2114:2017, SHA-512, base58, 90 chars (`c4` + 88), left-padded with `1`; **no IEEE standard number** | ST 2114 text fetched from `pub.smpte.org`; grep for "IEEE" returns only ISO/IEC/IEEE 9945 (POSIX) |
| MHL v1.1 uses `<file>`/`<size>`/`<lastmodificationdate>` as mandatory child elements, **no namespace**, **no `action`**, and distinguishes `<xxhash64>` (deprecated, little-endian) from `<xxhash64be>` | v1.1 XSD + real `.mhl` files showing the same file as `e338d99e661114f6` / `f61411669ed938e3` |
