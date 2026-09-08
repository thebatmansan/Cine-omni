# CINE-OMNI Industry Blueprint — 09: The Editorial Media Model

**Status:** research, Phase 2 input
**Date of research:** 2026-09-08
**Scope:** how professional NLEs actually model media identity, time, conform and
derived artefacts — and what that implies for CINE-OMNI's UCG.
**Companion documents:** [`02-editorial-media.md`](02-editorial-media.md) covers
the *engine* question (FFmpeg licensing, hardware decode, proxy plumbing). This
document covers the *model* question and deliberately does not repeat it.

> **Evidence discipline.** Every substantive claim below carries a primary-source
> URL — a published spec, a vendor documentation page, a DTD, or open-source code
> that was actually fetched on the date above. Where a claim could only be
> corroborated from community sources, or where a paywalled standard could not be
> read directly, it is marked **`UNVERIFIED`** and the limit of what is known is
> stated. Numeric claims about floating-point behaviour were *measured*, and the
> measurement is reproduced inline so it can be re-run.
>
> **This is a concept-discovery document.** It extracts the underlying ideas from
> public specifications and documentation. It reproduces no proprietary
> implementation detail, and nothing here is a design copied from a closed system.

---

## 0. The question, stated precisely

`cine-timeline` can already describe *an edit*. It cannot describe *a film's
media*. Those are different problems, and conflating them is the single most
common architectural mistake in this domain.

An edit is a set of decisions: this range of that thing, at this point, for this
long. A media model is an answer to a harder question — **what is "that thing",
such that the answer survives the file being moved, transcoded, proxied,
re-delivered at a different frame rate, colour-managed, archived to LTO, and
relinked eighteen months later on a different machine by a different person?**

Every serious system in this industry has had to answer that question, and — as
Section 1.8 shows — they converge, independently, on the same two-level answer.
CINE-OMNI currently has neither level.

---

## 1. The media identity chain

### 1.1 The layers that need distinguishing

Nine things get casually called "the clip". A real system has to tell them apart:

| # | Layer | What it is | Survives what |
|---|---|---|---|
| 1 | **Physical source** | The card, reel, roll, tape, or camera magazine | Everything; it is a real object |
| 2 | **Capture event** | One camera roll / take / recording session | Copying, transcoding |
| 3 | **Material** | The *content* — "this take of this shot" | Transcode, proxy, move, rename, re-wrap |
| 4 | **Representation** | One encoding of that material (camera original, ProRes transcode, proxy, DNx offline) | Being moved on disk |
| 5 | **File** | One byte-sequence at one path | Nothing; paths are the least durable identifier in post |
| 6 | **Container** | The wrapper (MXF, MOV, MP4, WAV) inside a file | — |
| 7 | **Elementary stream** | One video/audio/data track inside the container | — |
| 8 | **Catalogue entry** ("master clip", "asset", "bin clip", "producer") | The project's handle on the material, carrying the user's metadata | Reorganising the timeline |
| 9 | **Instance** | One placement of a trimmed range on a timeline | Being trimmed |

Plus two derived layers that behave like media but are *outputs*:

| # | Layer | What it is |
|---|---|---|
| 10 | **Derived/rendered version** | A render, a bake, a cache segment, an optimised media file |
| 11 | **Delivery** | A conformed master at a specific rate/colour/audio configuration |

### 1.2 AAF / Avid — the most rigorous public model

AAF is the reference answer, and it is a *published* answer. The AAF Object
Specification is at <https://static.amwa.tv/ms-01-aaf-object-spec.pdf> (mirror:
<https://aaf.sourceforge.net/docs/aafObjectModel.pdf>). Direct PDF text
extraction failed on both (compressed streams), so the object model below is
established from the AAF COM API reference documentation
(<https://aaf.sourceforge.net/docs/aafapiman/>) and from `pyaaf2`, a maintained
open-source AAF implementation (<https://pyaaf.readthedocs.io/>), both of which
are generated from the same object model. **Exact spec section numbers and
verbatim spec prose are `UNVERIFIED`.**

AAF's central abstraction is the **Mob** (Metadata OBject), of which there are
three kinds, chained:

```
CompositionMob        the edit — "play MobID X, slot 1, from frame 240, for 96 frames"
    │  SourceClip(sourceID = MobID, sourceTrackID = SlotID, startTime)
    ▼
MasterMob             the catalogue entry / "master clip" — a stable handle
    │  SourceClip
    ▼
File SourceMob        one file's essence, described by a FileDescriptor
    │  SourceClip
    ▼
Tape/Film SourceMob   the physical original, described by a TapeDescriptor
```

* **`IAAFSourceClip`**: *"The IAAFSourceClip interface is implemented by objects
  that reference the content data and identify the source of the content data"*
  and *"the 3 properties of a source Clip that make up the 'source reference' are
  sourceID, sourceTrackID, and startTime."*
  — <https://aaf.sourceforge.net/docs/aafapiman/interfaceIAAFSourceClip.html>

  This is the whole idea in one sentence: **a reference is (identity, slot,
  offset)**, never a path.

* **`IAAFMasterMob`**: *"The IAAFMasterMob interface is implemented by objects
  which provide access to the File Source Mobs and EssenceData objects."*
  `AddMasterSlot` *"adds a slot to the specified Master Mob that references the
  specified a slot in the specified Source Mob"*; the new slot *"contains a
  Source Clip specifying the Source Mob in its source reference properties."*
  — <https://aaf.sourceforge.net/docs/aafapiman/interfaceIAAFMasterMob.html>

  The MasterMob exists **purely as a layer of indirection**. It holds no essence
  and describes no format. Its entire job is to give the composition something to
  point at that does not change when the file underneath changes.

* **`IAAFSourceMob`**: a SourceMob carries *"some form of AAFEssenceDescriptor,
  either an AAFTapeDescriptor, AAFFileDescriptor AAFFilmDescriptor, or some
  extended descriptor."*
  — <https://aaf.sourceforge.net/docs/aafapiman/interfaceIAAFSourceMob.html>

  The descriptor hierarchy (verified against `pyaaf2`'s essence module,
  <https://pyaaf.readthedocs.io/en/latest/api/aaf2.essence.html>) is
  `EssenceDescriptor` → {`FileDescriptor` → `DigitalImageDescriptor` →
  {`CDCIDescriptor`, `RGBADescriptor`}, `TapeDescriptor`, `FilmDescriptor`,
  `PhysicalDescriptor` → `ImportDescriptor`}. `CDCIDescriptor` *"specifies that a
  file SourceMob is associated with video essence formatted with one luminance
  component and two color-difference components"*; `RGBADescriptor` adds
  `pixel_layout`. A file's location is carried by a `Locator` on the descriptor,
  not by the Mob.

* **MobID = SMPTE UMID.** `pyaaf2`'s MobID module implements SMPTE ST 330:
  a Basic UMID is *"A 12-byte universal label, A 1-byte length value, A 3-byte
  instance number, and A 16-byte material number."* The load-bearing sentences:

  > *"The purpose of the instance number is to separately identify different
  > representations or instances of audiovisual material."*
  >
  > *"The material number does not indicate the status of the material…The
  > material number can be identical in copies and in different representations."*

  — <https://pyaaf.readthedocs.io/en/latest/api/aaf2.mobid.html>
  (registration authority: <https://www.smpte-ra.org/umid-information>; the
  standard itself is paywalled — <https://standards.globalspec.com/std/1396751/smpte-st-330>
  — so the numeric material-number generation-method table is **`UNVERIFIED`**.)

  **This is the concept.** Identity is split into a *material number* that is
  invariant across every copy, transcode and representation, and an *instance
  number* that distinguishes them. Not a path. Not a hash of the bytes. A
  deliberate, two-level identifier where one level means "the same content" and
  the other means "this particular encoding of it".

* **Avid's relink infrastructure** rests on this: `msmMMOB.mdb` and `msFMID.pmr`
  in the `Avid MediaFiles` folder *"are used by Avid products to track the media
  file contents of these folders"*
  (<https://kb.avid.com/pkb/articles/en_US/troubleshooting/en262645>). Relinking
  is a MobID→file index rebuild, not a filename search. The binary format of
  those files is **`UNVERIFIED` / not publicly specified**.

* **Subclips and retimes** appear in AAF as recognisable CompositionMob patterns
  rather than new classes: LibAAF documents support for *"SubClip
  CompositionMobs"* and *"AdjustedClip CompositionMobs"*
  (<https://github.com/agfline/LibAAF>). **Avid multicam group clips have no
  publicly documented native AAF representation** — the community evidence
  (<https://community.avid.com/forums/t/114443.aspx>) is that multicam must be
  committed/flattened before AAF export. Marked **`UNVERIFIED`**; treat group
  clips as an application-level construct that AAF does not carry.

### 1.3 FCPXML — the same answer, arrived at independently, and cleanly

Apple's model is publicly documented and is the cleanest expression of the idea.
Crucially, **Apple changed it mid-life in exactly the direction AAF already
pointed**, and the change is visible in Apple's own DTDs.

FCPXML v1.8 (<https://raw.githubusercontent.com/CommandPost/CommandPost/develop/src/extensions/cp/apple/fcpxml/dtd/FCPXMLv1_8.dtd>):

```dtd
<!ELEMENT asset (bookmark?,metadata?)>
<!ATTLIST asset uid CDATA #IMPLIED>
<!ATTLIST asset src CDATA #REQUIRED>        <!-- file: URL -->
```

FCPXML v1.11 (<https://raw.githubusercontent.com/CommandPost/CommandPost/develop/src/extensions/cp/apple/fcpxml/dtd/FCPXMLv1_11.dtd>):

```dtd
<!ELEMENT asset (media-rep+, metadata?)>
<!ATTLIST asset uid CDATA #IMPLIED>
<!-- note: no src attribute -->

<!ELEMENT media-rep (bookmark?)>
<!ATTLIST media-rep kind (original-media | proxy-media) "original-media">
<!ATTLIST media-rep sig CDATA #IMPLIED>     <!-- assigned by FCP -->
<!ATTLIST media-rep src CDATA #REQUIRED>
<!ATTLIST media-rep suggestedFilename CDATA #IMPLIED>
```

**The path moved off the identity and onto the representation, and the asset
gained a one-to-many relationship to its representations.** Apple's prose
confirms the intent:

* `asset`: *"Use the `asset` element to define a file-based media. A file-based
  media can have an original media representation and a proxy media
  representation. Describe those using the `media-rep` element along with file
  URLs for the media files."*
* `asset/@uid`: *"The unique identifier of a media. By default, Final Cut Pro
  assigns a unique value. Third-party tools may assign their unique identifiers
  for media managed by their tool… Use a UUID or a string with a reverse DNS
  prefix to ensure uniqueness."*
  — <https://developer.apple.com/documentation/professional-video-applications/asset>
* `media-rep`: *"References a media representation, that is either the original
  or a proxy media managed by Final Cut Pro."*
  `media-rep/@sig`: *"The unique identifier of a media representation, assigned
  by Final Cut Pro."*
  — <https://developer.apple.com/documentation/professional-video-applications/media-rep>

Apple states the *purpose* of `uid` in terms that could serve as the requirements
statement for CINE-OMNI Phase 2:

> *"Use the `uid` attribute to reference the same asset in a subsequent XML
> import, even if the media representation is in a different location. Final Cut
> Pro locates the asset using the `uid` attribute value and relinks the media to
> the media file at the new location."*
>
> *"The `uid` attribute enables a workflow where the media initially imports as
> offline with its media file missing, and subsequently relinks to the media file
> when it becomes available."*
>
> — <https://developer.apple.com/documentation/professional-video-applications/asset>

Note the exact mechanism: **the identifier locates the asset, and the file
location is what gets rebound.** That is the inverse of a path-keyed model, where
the location locates the asset and there is nothing left to rebind.

`asset/@uid` is AAF's material number. `media-rep/@sig` is AAF's instance number.
Two industry-defining systems, twenty years apart, different companies, no shared
code — **the same two-level split.**

The rest of the FCPXML model is equally disciplined, and the discipline is always
the same: **identity lives in `<resources>`; instances live in the story.**

| `<resources>` (identity, has an `id`) | Story element (instance, has a `ref`) |
|---|---|
| `asset` — file-based media, `uid` + `media-rep+` | `asset-clip` — *"References a single media asset."* `ref` → `asset` ID |
| `media` + `<sequence>` — a compound clip's construction | `ref-clip` — *"References a compound clip media."* `ref` → `media` ID |
| `media` + `<multicam>` — angles as a unit | `mc-clip` — *"References a multicam media."* `ref` → `media` ID |
| `format` — frameDuration, field order, dimensions, colour space | referenced by `format` IDREF from asset, sequence, clip, spine |
| `effect` | `filter-video` / `filter-audio` / `title` |

— element descriptions verbatim from
<https://developer.apple.com/documentation/professional-video-applications/story-elements>
and <https://developer.apple.com/documentation/professional-video-applications/media>;
`ref IDREF #REQUIRED` on `asset-clip`, `ref-clip`, `mc-clip` verified in the DTD.

Three things follow that a data model must respect:

1. **A compound clip and a multicam clip are *media*, not timeline items.**
   `<!ELEMENT media (multicam | sequence)?>`. They sit in the resource pool
   alongside `asset` and are referenced the same way. A nested sequence is a
   first-class piece of *identity* that many instances can point at — not a
   subtree copied into a track.
2. **Format is a shared resource, not a clip property.** `frameDuration`,
   `fieldOrder`, `width`, `height`, `paspH/paspV`, `colorSpace` live on a
   `format` that assets, sequences, clips and spines all point at by IDREF
   (<https://developer.apple.com/documentation/professional-video-applications/format>).
   The same format description is shared, not duplicated per clip.
3. **`sync-clip` is a container, not a property.** *"Contains a clip with its
   contained and anchored items synchronized."* Dual-system sync is modelled as a
   composite identity, not as an offset attribute smeared over two clips.

### 1.4 MLT / Kdenlive — producers as identity

MLT collapses the layers further but keeps the essential one. Its object model is
Producer / Playlist / Tractor / Multitrack / Filter / Transition / Consumer:
*"Playlists are sequential containers for producers and blank spaces"*;
*"Multitracks are parallel containers for multiple producers that act as a single
producer"*; *"tractors pull frames from producers and then run them through
filters and transitions, with the result then pushed into the consumers"*
(<https://mltframework.org/docs/mlt++/>).

The identity mechanism is in the DTD
(<https://github.com/mltframework/mlt/blob/master/src/modules/xml/mlt-xml.dtd>):

```dtd
<!ATTLIST producer id ID #IMPLIED  ... mlt_service CDATA #IMPLIED>
<!ATTLIST entry producer IDREF #IMPLIED in CDATA #IMPLIED out CDATA #IMPLIED>
<!ATTLIST track producer IDREF #IMPLIED hide (video|audio|both) #IMPLIED>
```

`entry/@producer` and `track/@producer` are `IDREF`. That is formal proof — not a
documentation aside — that **one `<producer>` is the shared identity referenced
by many playlist entries**, each carrying its own `in`/`out`. Producer = master
clip; entry = instance. The `resource` property carries the path and
`mlt_service` names the module that can open it (e.g. `avformat`); since MLT
21.05 `<chain>` replaces `<producer>` for `avformat` sources.

Kdenlive layers proxies on top by convention: a `kdenlive:proxy` property on a
bin clip signals a generated proxy, and proxies are cached under
`$HOME/.cache/kdenlive/proxy/` keyed by a hash of the source. The Project Bin
displays *"the number of instances in sequences (first number) and the timeline
(second number)"* and a *"Clip in Timeline"* menu that *"opens a flyout with a
list of all the instances of the selected clip in the timeline"*
(<https://docs.kdenlive.org/en/project_and_asset_management/project_bin/project_bin_use.html>),
which is the bin-clip↔instance relation made visible in the UI. Details of the
`kdenlive:proxy` property and offline-clip state are **`UNVERIFIED`** from the
manual and live only in source (<https://invent.kde.org/multimedia/kdenlive>).

**Licences** (relevant because the licensing spine forbids linking): MLT is
LGPL-2.1 for the framework with GPL CLI tools
(<https://github.com/mltframework/mlt/blob/master/COPYING>,
<https://www.mltframework.org/docs/copyrightpolicy/>); Kdenlive is GPL-3.0
(<https://github.com/KDE/kdenlive/blob/master/COPYING>). Study only; never link.

### 1.5 Where OTIO sits

OTIO models layers 8→9 and points weakly at 5. A `Clip` *"contains a
MediaReference and a trim on that media reference"*; an `ExternalReference` has a
`target_url`; `available_range` is *"a TimeRange object which specifies a
start_time and duration… Can be None if media is missing or expensive to query"*
(<https://opentimelineio.readthedocs.io/en/latest/api/python/opentimelineio.schema.html>,
<https://opentimelineio.readthedocs.io/en/latest/tutorials/otio-timeline-structure.html>).

OTIO *does* have the representation concept — and it is the part CINE-OMNI
missed. A `Clip` carries **multiple** media references:

* `DEFAULT_MEDIA_KEY = 'DEFAULT_MEDIA'`
* `media_references() -> dict[str, MediaReference]`
* `active_media_reference_key`
* `set_media_references(Mapping[str, MediaReference], str)`

— <https://opentimelineio.readthedocs.io/en/latest/api/python/opentimelineio.schema.html>

That dictionary keyed by a string, with one key marked active, *is* the
proxy/original switch: same clip, same edit, different representation. It has no
material-level identity above it (there is no `uid`), but the representation
level is there.

### 1.6 DaVinci Resolve and Premiere Pro — the same shape in a product idiom

Neither publishes a schema, so this section is deliberately limited to *behaviour
documented by the vendor*. No internal design is asserted.

**Resolve** (verified against Blackmagic's own training PDFs:
<https://documents.blackmagicdesign.com/UserManuals/DaVinci-Resolve-20-Editors-Guide.pdf>,
<https://documents.blackmagicdesign.com/UserManuals/DaVinci-Resolve-16-Beginners-Guide.pdf>):

* **Master clip vs timeline clip is an explicit, navigable relation.** *"Match
  Frame will open a timeline clip's master clip in the source viewer, displaying
  the same frame and In and Out points as the timeline clip."* An instance knows
  its material and can be traced back to it, frame-exactly.
* **Representation choice is a three-state project-level switch:** the *Proxy
  Handling* menu offers *"Prefer Camera Originals"* (default), *"Prefer Proxies"*
  and *"Disable All Proxies"*. One control, project-wide, no edit touched.
* **Offline is a resolution state, not damage:** *"Change the Proxy Handling menu
  to Disable Proxies. All the media in the bin now displays as offline since the
  original media is not available in this location."* The clips are intact; only
  resolution failed.
* **Relink is a re-resolution operation:** *"right-click them, and choose Clip
  Operations > Change Source Folder… Instantly, DaVinci Resolve looks to the new
  location for the media… The media instantly relinks to the original files."*
  Bin-level: *"Relink Clips for Selected Bins."*
* **Subclips are references, not copies:** *"Subclips are discrete clips that have
  been isolated from a much longer clip, but because they are still referencing
  the original media file, they are not taking up any additional storage space."*
  And editing a subclip updates *"the limits of the subclip in both the media pool
  and the timeline simultaneously"* — the subclip is an identity that instances
  follow, exactly as §4.1 argues.
* **Clip Attributes** are per-material overrides of intrinsic properties (notably
  audio channel mapping) — i.e. the project may correct what the file claims about
  itself without modifying the file. This is a real requirement: **the material's
  description must be authorable, not merely probed.**
* **Smart Bins** *"search your entire project to group clips based on metadata that
  you define"* and are *"updated in real-time"* — a stored query, not a folder.
  **Power Bins** are *"available across every project within the current project
  library"*, but *"Multicam clips, compound clips, timelines, and Fusion clips
  cannot be placed in power bins"* — composite identities are project-scoped while
  material identities are library-scoped, which is itself a statement about which
  layer is portable.
* **Media Management** *"allows you to copy or transcode the media for the entire
  project, specific timelines, or specified clips"*, with trim-with-handles
  (*"plus an additional 24 frames before the In point and after the Out point"*) —
  consolidation is a representation-level operation that must preserve material
  identity or the project breaks.

**Premiere Pro.** `helpx.adobe.com` blocks direct fetches from this environment
(TLS connects, then the request hangs) via both the fetch tool and `curl`; a
reader-proxy fetch path eventually got through for some pages, so the quotes
below are verbatim and cited, and the rest is marked `UNVERIFIED` explicitly
rather than blended in.

*Verified verbatim:*

* **Subclips are created from a timeline instance, not only from source media** —
  *"Hold Ctrl (Windows) or Command (macOS) on your keyboard and click-drag the
  clip from the sequence to an open bin in the Project panel… Alternatively, you
  can also right-click a clip instance in a sequence and select Make Subclip."*
  A subclip converts back to independent material on demand: *"Select the
  subclip in the Project panel. Select Clip>Edit Subclip… Select Convert To
  Master Clip."* The subclip *"retains the master clip start and end times"* —
  i.e. it is created from and remains tied to its source's own timebase, matching
  §4.1's "subclip is `DerivedFrom` its parent material" design.
  — <https://helpx.adobe.com/premiere/desktop/edit-projects/intro-to-editing/create-a-subclip-from-the-timeline.html>,
  <https://helpx.adobe.com/premiere/desktop/edit-projects/intro-to-editing/convert-subclip-to-master-clip.html>
* **Multicam is built by grouping unrelated source clips, with an explicit
  choice about whether non-overlapping ones survive:** *"Select the clips in the
  Project panel and select Clip>Create Multi-Camera Source Sequence."* *"Select
  Create Single Multicam Source Sequence to combine the clips into a single
  multi-camera sequence… If you don't choose this option, only overlapping clips
  are combined, and clips with no overlaps are left unused."* This is a real
  design decision a sync-group model must expose: partial-coverage angles are not
  silently dropped, they are an explicit user choice.
  — <https://helpx.adobe.com/premiere/desktop/edit-projects/set-up-multi-camera-sequences-for-editing/create-a-multi-camera-source-sequence.html>
* **Nested sequences are referenced, not copied, and this is stated as the whole
  point:** *"A nested sequence is a sequence placed within another sequence…
  Each nested sequence appears as a single, linked video or audio clip, even
  though its source may include multiple tracks with different timebases, frame
  sizes, or pixel aspect ratios."* *"Any change you make to the source sequence
  automatically updates in all its nested instances"* while *"Trimming affects
  only the nested clip, not the source."* This is §4.2's Composite/Instance split
  stated by the vendor almost word for word: shared identity, independent
  per-instance trim.
  — <https://helpx.adobe.com/premiere/desktop/edit-projects/edit-nested-sequences/about-nested-sequences.html>
* **Relink degrades gracefully through weakened match criteria, and the vendor
  documents the reasoning:** *"Changed file formats (for example, .mov to .mp4):
  If you have transcoded media, then unchecking the File Extension option in the
  Match File Properties section of the linking dialog box should enable linking
  to the new files."* Premiere also tracks *"current path + 2 history slots"* for
  standalone projects (unlimited for Team Projects) and runs a two-pass folder
  search (project folder and subfolders, then the parent folder) before falling
  back to the manual Link Media dialog — i.e. relink is a **cascade of
  progressively weaker automatic strategies** before the user is asked to choose
  a match key by hand, which is the operational version of §3.2's match-key
  table.
  — <https://helpx.adobe.com/premiere/desktop/organize-media/file-organization/relink-offline-media-automatically.html>

*Still `UNVERIFIED`* (pages 404'd, hung, or were only ever seen as search
snippets — not independently fetched): the full **Match File Properties**
checkbox set beyond `File Extension` (search snippets suggest *File Name, Media
Start, Tape Name* also exist, and that *"the combination… you select should
uniquely identify each media file"*, but this is not confirmed verbatim); the
*Attach Proxies* / *Toggle Proxies* proxy-workflow pages; the merged-clips page;
and the term "Restrict Trims to Subclip Boundaries", which could not be located
on any Adobe page and may not be the current UI term.

The verified material still supports the reading in §3.2: Premiere's relink is a
**cascade of weak, attribute-based match keys** rather than a persistent material
identifier — closer to EDL/CMX3600's reel+timecode matching than to AAF's MobID
or FCPXML's `asset/@uid`.

### 1.7 Comparison table — the identity chain across seven systems

| Concept | AAF / Avid | FCPXML | Premiere Pro | DaVinci Resolve | MLT / Kdenlive | OTIO | **CINE-OMNI today** |
|---|---|---|---|---|---|---|---|
| Physical source | `SourceMob` + `TapeDescriptor` / `FilmDescriptor` | `metadata` / reel keys | Tape Name match field `UNVERIFIED` | reel / camera metadata | — | metadata only | **none** |
| Material identity (invariant) | **MobID material number** | **`asset/@uid`** | master clip *(no persistent ID documented — relink uses a composite of weak keys)* `UNVERIFIED` | Media Pool master clip (Match Frame resolves instance → master) | `producer/@id` (project-local) | *(none)* | **none** |
| Representation (per-encoding) | **MobID instance number**, File `SourceMob` | **`media-rep` + `@sig`, `@kind`** | Attach Proxies / Toggle Proxies `UNVERIFIED` | proxy media + optimised media; project-level *Prefer Camera Originals / Prefer Proxies / Disable All Proxies* | `kdenlive:proxy` property | `media_references()` + `active_media_reference_key` | **none — one path, one node** |
| File / path | `Locator` on the descriptor | `media-rep/@src` | file path | file path (rebound by *Change Source Folder*) | `resource` property | `ExternalReference.target_url` | `asset.media.media_path` **(= the identity)** |
| Container / stream description | `FileDescriptor`, `CDCIDescriptor`, `RGBADescriptor` | `format` resource + `hasVideo`/`hasAudio`/`audioSources`/`audioChannels`/`audioRate` | clip properties `UNVERIFIED` | **Clip Attributes** — per-material authorable overrides | producer properties | *(none — out of scope)* | **none** |
| Catalogue entry / master clip | **`MasterMob`** | `asset` / `media` in `<resources>` | master clip `UNVERIFIED` | Media Pool master clip | `<producer>` | *(none)* | `asset.media` (conflated with the file) |
| Composite identity (compound / multicam) | CompositionMob | `media` + `sequence` / `multicam` | nested sequence / multi-camera source sequence / merged clip `UNVERIFIED` | compound clip / multicam / Fusion clip (project-scoped: cannot go in Power Bins) | `tractor` as a producer | nested `Stack` **(inline, not referenced)** | nested `Stack` (inline) |
| Instance on a timeline | `SourceClip` in a CompositionMob | `asset-clip` / `ref-clip` / `mc-clip` | timeline clip `UNVERIFIED` | timeline clip | `<entry producer=… in= out=>` | `Clip` | `nle.clip` |
| Derived / rendered version | new SourceMob | render files, `renderFormat` | preview renders `UNVERIFIED` | render cache, optimised media, Media Management transcodes | consumer output | *(none)* | **none** |



### 1.8 The convergent concept

Strip the vocabulary away and four independent designs say the same thing:

> **Identity is two-level. A *material* identifier names the content and is
> invariant under copy, transcode, proxy, re-wrap, rename and move. A
> *representation* identifier names one concrete encoding of that material and is
> what actually resolves to bytes. The timeline references only the material.**

AAF calls them material number and instance number. FCPXML calls them `uid` and
`sig`. OTIO half-implements the second level as a keyed dictionary. MLT gets the
first level via `IDREF` but has no second level. Everything else — proxies,
relinking, offline/online, media management, optimised media, LTO archive — is a
*consequence* of getting these two right.

**The corollary that matters most for CINE-OMNI: a file path is a
representation-level attribute, never an identity.** A system that derives the
identity of media from its path (as `EntityId::from_key("media:<url>")` does) has
made the one mistake that no professional system makes, and it makes it in the
place that is hardest to change later — the identity function.

---

## 2. Time

### 2.1 Four clocks, not one

Editorial has at least four distinct time coordinate systems, and most timeline
bugs are a confusion between two of them.

| Clock | Origin | Unit | Whose | Example |
|---|---|---|---|---|
| **Sample / frame time** | first sample of the essence | frames or audio samples, integer | the decoder's | "frame 1,204 of this file" |
| **Source time** | the material's own zero | rational at the material's rate | the material's | `available_range`, `source_range` |
| **Timeline time** | the sequence's zero | rational at the sequence's rate | the edit's | where a clip sits on a track |
| **Timecode** | an arbitrary labelled origin | `HH:MM:SS:FF`, a *label* | the production's | `01:00:00:00` start of programme |

Timecode is the one that is misunderstood. **Timecode is a display encoding of a
frame index, not a time.** It has an origin (`tcStart`), a format (drop or
non-drop), and a rate — and the same frame index yields different timecode
strings under different formats. FCPXML is explicit about this: `tcStart` is
*"clip timecode origin"* and `tcFormat (DF | NDF)` is *"timecode display format
(DF=drop frame; NDF=non-drop frame)"* — attributes of the clip, carried alongside
the actual times, which are rationals (verified in the FCPXML 1.8 DTD, lines
456–457 and 475–476). AAF likewise keeps a separate `Timecode` segment;
`IAAFTimecode` *"is implemented by objects which store videotape or audio tape
timecode information"* with a `Timecode (startFrame, drop, fps)` structure
(<https://aaf.sourceforge.net/docs/aafapiman/interfaceIAAFTimecode.html>) — a
`drop` flag alongside a frame number, never a formatted string as the value.

### 2.2 How each system represents rational time

| System | Representation | Source |
|---|---|---|
| **FCPXML** | *"'time' attributes are expressed as a rational number of seconds (e.g., "1001/30000s") with a 64-bit numerator and 32-bit denominator. Integer 'time' values, such as 5 seconds, may be expressed as whole numbers (e.g., '5s')."* — **verbatim from Apple's own DTD** | [FCPXMLv1_8.dtd, lines 6–8](https://raw.githubusercontent.com/CommandPost/CommandPost/develop/src/extensions/cp/apple/fcpxml/dtd/FCPXMLv1_8.dtd) |
| **AAF** | `EditRate` is a `Rational` (numerator/denominator); positions are integer edit units at that rate | AAF object model; exact spec prose **`UNVERIFIED`** (PDF extraction failed) |
| **MLT** | Frames are the native unit. Profiles carry `frame_rate_num`/`frame_rate_den`: *"A num/den lets you specify a fraction without rounding errors introduced by floating point."* HDV 720p30 ships as `frame_rate_num=30000 frame_rate_den=1001` | <https://www.mltframework.org/docs/profiles/>, <https://github.com/mltframework/mlt/blob/master/profiles/hdv_720_30p> |
| **OTIO** | *"The RationalTime class represents a measure of time of rt.value/rt.rate seconds."* But both `value` and `rate` are typed `SupportsFloat \| SupportsIndex` — **floats are accepted** | <https://opentimelineio.readthedocs.io/en/latest/api/python/opentimelineio.opentime.html> |
| **CINE-OMNI** | `RationalTime { value: f64, rate: f64 }` — matches OTIO's C++ (`double`), including its weakness | `crates/ucg-core/src/time.rs` |

Note what FCPXML does that the others do not: it expresses the *frame duration*
(`format/@frameDuration`, *"The frame duration as a time value"*) rather than a
frame rate. `1001/30000s` is exact as a rational duration; `29.97` is exact as
nothing. Storing the reciprocal removes an entire class of "is this rate 29.97 or
30000/1001?" ambiguity.

### 2.3 Why floating-point is the wrong authoritative representation — measured

The usual objection to floats is hand-waved ("rounding errors"). It is worth
being exact, because the actual failure mode is not drift; **it is that
`floor()` returns the wrong frame.**

`ucg_core::RationalTime` stores `value: f64, rate: f64`. `rescaled_to` computes
`(value / rate) * new_rate`; `to_frames` computes `value.floor()`. `Add`/`Sub`
rescale the right-hand side to the left-hand rate. `Track::rate()` takes the rate
of *the first timed child* and sums everything at that rate. So any track holding
clips at two rates performs a float rescale per item.

Measured on this machine (script reproduced below, IEEE-754 binary64, matching
Rust's `f64`):

**(a) One rescale round-trip through a 24 fps track puts 8–13 % of frame indices
on the wrong frame.**

| Clip rate | Frames landing on the *wrong* integer after clip-rate → 24 → clip-rate, of 300,000 | First failures |
|---|---|---|
| 23.976 (24000/1001) | **39,489 (13.2 %)** | frame 1 → `0.9999999999999999`; frame 2 → `1.9999999999999998` |
| 29.97 (30000/1001) | **27,496 (9.2 %)** | frame 5 → `4.999999999999999` |
| 25 | **37,970 (12.7 %)** | frame 29 → `28.999999999999996` |
| 30 | **28,403 (9.5 %)** | frame 7 → `6.999999999999999` |

Read that again: **frame 1 of a 23.976 clip, placed on a 24 fps track and read
back, floors to frame 0.** This is not a long-timeline effect and it is not
confined to NTSC rates — integer rates fail too, because `(1/24)*24` is not the
problem; `(1/23.976023976023978)*24 → /24 * 23.976023976023978` is.

**(b) It compounds through nesting.** A 23.976 clip inside a 24 fps track inside a
29.97 timeline, read back: **29,714 of 200,000 frames (14.9 %) land on the wrong
frame** — first failure again at frame 1.

**(c) A 10-hour timeline's last frame is not representable.** Frame 863,136 at
23.976 → `35999.964` s → back to `863136.0000000001`. Not an integer. Any
equality test, any `HashMap` key, any "is this the last frame" check is now
rate-dependent and machine-dependent.

**(d) Accumulated drift is *not* the danger.** 20 million float additions of a
rescaled one-frame step accumulate only `1.16e-3` frames of error. The hazard is
entirely in *comparison and truncation*, which is why the existing code is
already littered with `1e-9` epsilons (`Track::validate`, three sites) and an
`almost_equal(eps_seconds)` method. **Epsilons in a frame-accurate system are a
smell: they are a workaround for a representation that cannot answer "is this the
same frame?" exactly.**

<details>
<summary>Reproduction script</summary>

```python
def rescale(v, r, nr): return (v / r) * nr          # exactly RationalTime::rescaled_to

for rate, label in [(24000/1001, '23.976'), (30000/1001, '29.97'), (25.0, '25'), (30.0, '30')]:
    bad = [(f, v) for f in range(300_000)
           for v in [rescale(rescale(float(f), rate, 24.0), 24.0, rate)]
           if int(v // 1) != f]
    print(label, len(bad), bad[:3])
```
</details>

**The correct representation** is the one AAF, MLT and FCPXML all use: an exact
rational — `value: i64` (or `i128`) frames at `rate: (u32, u32)` numerator and
denominator, with rescaling done by exact integer arithmetic and an *explicit,
named rounding policy* at the only place rounding is legitimate (converting a
position from one rate to another for display or for a decode request). OTIO's
own history is evidence: `nearest_valid_timecode_rate` had to be introduced and
is now deprecated in favour of `nearest_smpte_timecode_rate` — *"This function
will be removed in a future release"*
(<https://opentimelineio.readthedocs.io/en/latest/api/python/opentimelineio.opentime.html>).
A system that had exact rates would not have needed a function to snap arbitrary
rates back to legal ones.

### 2.4 Drop-frame — and a live defect in `to_timecode()`

29.97 is exactly 30000/1001 fps. Counting frames as if they were 30 fps runs the
label fast against the wall clock by a factor of 1001/1000. Drop-frame timecode
compensates by **skipping frame *numbers* 0 and 1 at the start of every minute,
except every tenth minute** (00, 10, 20, 30, 40, 50) — 18 skipped numbers per ten
minutes. No frames are dropped; only labels are. The residual error is about
86.4 ms (≈2.59 frames) per day. SMPTE ST 12-1 governs the encoding
(<https://www.smpte.org/blog/understanding-standards-time-code>,
<https://standards.globalspec.com/std/1674717/smpte-st-12-1>,
<https://en.wikipedia.org/wiki/SMPTE_timecode>).

`RationalTime::to_timecode()` in `crates/ucg-core/src/time.rs` documents itself as
*"SMPTE non-drop timecode string"* and computes `let rate = self.rate.round()`.
At 29.97 that is 30, and there is no drop-frame path at all — `from_timecode`
accepts `;` as a separator but treats it identically to `:`. Measured consequence
of labelling 29.97 material with non-drop timecode:

| Real elapsed | Frames | Correct drop-frame | What `to_timecode()` prints | Label error |
|---|---|---|---|---|
| 1 minute | 1,798 | `00:00:59;28` | `00:00:59:28` | 0.07 s |
| 10 minutes | 17,982 | `00:09:59;28` | `00:09:59:12` | 0.60 s |
| **1 hour** | 107,892 | `00:59:59;28` | `00:59:56:12` | **3.60 s (108 frames)** |
| 3 hours | 323,676 | `02:59:59;28` | `02:59:49:06` | 10.80 s (324 frames) |

For a broadcast deliverable this is a rejected master. The honest framing:
**`to_timecode()` is correct for what it claims (non-drop) and the defect is that
`RationalTime` has nowhere to record whether drop-frame is the material's
convention.** Drop-vs-non-drop is not a property of a time value; it is a
property of the *timecode track* the value is labelled against — which is exactly
why FCPXML puts `tcFormat` on the clip and AAF puts `drop` in the Timecode
segment, and why the fix is a `Timecode`/`TimecodeTrack` concept, not a flag on
`RationalTime`.

### 2.5 Variable frame rate

VFR breaks the constant-timebase assumption every NLE is built on. Container
support is real: ISO/IEC 14496-12 (ISO base media / MP4) encodes decode times in
the `stts` (decoding time-to-sample) box as `DT(n+1) = DT(n) + STTS(n)`, a list of
(sample count, sample delta) entries; a single entry means constant rate, and
multiple entries express genuinely variable per-sample durations
(<https://b.goeswhere.com/ISO_IEC_14496-12_2015.pdf>,
<https://www.iso.org/obp/ui/#iso:std:iso-iec:14496:-12:ed-4:en>). Phone and screen
capture produce these routinely.

The universal industry response is **conform to CFR at ingest** — decide a
constant rate, resample or duplicate/drop, and record what was done. This is a
modelling requirement, not just a decoder one: the material's *native* timing and
its *conformed* timing are different facts and both must be stored, or audio
drifts against video with no way to explain why. FCPXML makes the conform an
explicit, inspectable element rather than a hidden behaviour:

```dtd
<!ELEMENT conform-rate EMPTY>
<!ATTLIST conform-rate scaleEnabled (0 | 1) "1">
<!ATTLIST conform-rate srcFrameRate (23.98|24|25|29.97|30|60|47.95|48|50|59.94) #IMPLIED>
<!ATTLIST conform-rate frameSampling (floor|nearest-neighbor|frame-blending|optical-flow-classic|optical-flow) "floor">
```

*"Apply rate conforming to match media frame rate to timeline frame rate."*
— <https://developer.apple.com/documentation/professional-video-applications/conform-rate>,
DTD lines 635–638. Note that `frameSampling` — *how* frames are chosen — is part
of the edit decision, stored, and round-trips. That is the right instinct: the
resampling policy is authored data, not an implementation detail.

---

## 3. Conform and relink

### 3.1 What "offline" actually means

**Offline is not "the file is missing". Offline is "the material is known and its
representation is not currently resolvable."** That sentence only makes sense in a
system that has both levels of identity from §1.8. Without material identity there
is nothing left when the file goes away, so "offline" degenerates into "the clip
is broken" — which is precisely what happens in a path-keyed model.

Apple states the behaviour that follows from having both levels, and it is worth
reading carefully because it is the opposite of an error path:

> *"If the file is missing, Final Cut Pro still imports the media representation
> and marks it offline. Red frames indicate that the media is missing."*
>
> — <https://developer.apple.com/documentation/professional-video-applications/media-rep>

**The import succeeds.** The representation is recorded along with the fact that
it does not currently resolve. Offline is a stored state on a real object, not a
failure to construct one. Resolve behaves the same way — disabling proxies makes
clips *"display as offline"* rather than break the timeline (§1.6).

The operational states are:

| State | Meaning | What the system still knows |
|---|---|---|
| **Online** | Material has a resolvable, verified representation | everything |
| **Offline** | Material exists; no representation resolves right now | the edit, the ranges, the metadata, the identity |
| **Unresolved / needs conform** | Material referenced by an imported edit that has never been matched to anything local | reel, timecode, filename, whatever the interchange carried |
| **Substituted** | Resolving to a representation that is not the intended one (proxy standing in for original) | which one is active, and that it is a stand-in |
| **Mismatched** | A representation resolves but its properties contradict what the edit assumed (rate, duration, colour) | both the assumption and the reality |

The last one is the state that path-keyed systems cannot represent at all, and it
is the one that silently destroys conforms.

### 3.2 The four match keys, in order of strength

| Key | Strength | Survives | Fails when | Carried by |
|---|---|---|---|---|
| **Persistent material ID** (UMID / MobID / `uid`) | strongest | move, rename, transcode, re-wrap, proxy | the source system never assigned one, or the interchange drops it | AAF (MobID), MXF (UMID), FCPXML (`asset/@uid`), MOV/MP4 UUID boxes *(support varies — **`UNVERIFIED`**)* |
| **Content hash** | strong for exact bytes | move, rename | any transcode, any re-wrap, any partial file | FCPXML `media-rep/@sig` (*"assigned by Final Cut Pro"* — an FCP-internal signature; whether it is a content hash is **`UNVERIFIED`**), Kdenlive proxy cache keys |
| **Timecode + reel/tape** | the traditional conform key; works across formats and vendors | transcode, re-wrap, offline/online round trips | reel names collide, are 8 chars, or are absent on file-based media | EDL (reel + src in/out), AAF (`TapeDescriptor` + `Timecode`), XML |
| **Filename (± extension, ± duration)** | weakest, most used | nothing structural | duplicates, renames, camera-restarted numbering | every format |

The industry's honest position is that these are used **in cascade**, and that a
serious system must record *which key matched* — because "relinked by filename" is
a different level of confidence from "relinked by UMID", and the editor needs to
know which one they are trusting.

### 3.3 What each interchange format actually carries

**CMX 3600 EDL** carries, per event: event number (max 999), source reel/tape name
(**max 8 characters, A–Z and 0–9 only** — a holdover from hardware that no longer
exists), channel, edit type (`C` cut, `D` dissolve, `W` wipe), source in/out
timecode, and record in/out timecode. File-based sources with no real reel are
conventionally given reel `AX`. The actual clip identity survives only in comment
lines — `*FROM CLIP NAME:` and `*SOURCE FILE:` — which is why EDL conform is a
text-matching exercise
(<https://www.edlmax.com/edlmaxhelp/Edl/maxguide.html>,
<https://en.wikipedia.org/wiki/Edit_decision_list>; later formalised as SMPTE
258M). An EDL carries **no material identity, no effects, no nesting, no
multichannel audio mapping** — it carries cuts and timecode. That is its entire
value and its entire limitation.

**AAF** carries the MobID chain, the tape name via `SourceMob` + `TapeDescriptor`,
start timecode via a `Timecode` segment, and the file location via a `Locator` on
the `FileDescriptor`. Avid's export option *"Use tape sources when present"*
writes tape-name metadata so other applications can relink by tape name, falling
back to file name when absent — this is community/vendor-forum evidence
(<https://duc.avid.com/showthread.php?t=355581>) rather than spec text, so the
exact option semantics are **`UNVERIFIED`**. Practitioner accounts of
Resolve↔Avid round trips consistently report that *"three main pieces of metadata
make the roundtrip … timecode, clip name, and reel name ('tape' in Avid
nomenclature)"*
(<https://help.frameone.com/support/solutions/articles/69000800937-the-resolve-to-avid-round-trip>
— third-party, but practically authoritative for observed interop behaviour).

**FCPXML** carries `asset/@uid`, plus one `media-rep` per representation with its
`src`, `sig`, `kind` and an optional macOS security-scoped `bookmark`. It is the
only one of the three that carries the proxy relationship *as data* rather than as
a convention.

### 3.4 Proxies and optimised media: the invariant

The invariant every system enforces, however differently:

> **Swapping representations must not touch a single edit decision.**

The edit references the material; the material offers representations; a switch
changes which representation resolves. If the edit references the *file*, then
"switch to proxies" is a rewrite of the timeline — and a rewrite of the timeline
is a change to authored data, which means undo history, dirty state, collaboration
conflicts and provenance all become wrong for a purely operational choice.

This is the strongest argument for putting the material/representation split into
the graph rather than handling proxies in the player. In a UCG, a proxy switch
must produce **zero ops on any `nle.*` node**. If it produces ops, the model is
wrong.

Two further consequences that fall out of the same invariant:

* A representation's `available_range` and its **frame-index correspondence to the
  material** must be recorded. A proxy that starts at a different frame, or is
  conformed from VFR differently from the original, is not a valid stand-in — and
  it is only detectable if both representations describe their own timing.
* Optimised/render media is a *third* thing: a representation the system produced
  itself, from a known input, by a known operation. It is a **derived** artefact
  and belongs on a `DerivedFrom` edge so it can be invalidated automatically when
  its input changes. Proxies straddle this: user-supplied proxies are
  representations; system-generated proxies are derived representations, and
  knowing which is which decides whether they may be silently regenerated.

---

## 4. Multicam, subclips, compounds and retimes

These four are usually treated as features. They are actually four probes that
reveal whether a data model is sound, because each one breaks a naive model in a
different place.

### 4.1 Subclip — "is a range a first-class thing?"

A subclip is a named, bounded view of a material. The question it forces: **does
the model have a level between "the whole material" and "one instance on a
timeline"?** In FCPXML there is no separate subclip element — a subclip is
expressed as a `clip`/`asset-clip` with `start`+`duration`, i.e. as an instance
that may itself be referenced. In AAF, LibAAF reports subclips appear as
*"SubClip CompositionMobs"* (<https://github.com/agfline/LibAAF>) — that is, a
subclip is given **its own MobID and its own composition**. The AAF answer is the
principled one: if a thing can be organised, named, given metadata, and referenced
from multiple places, it needs an identity.

The design consequence: **a subclip is not an attribute of a clip; it is a
material-like node that `DerivedFrom` its parent material and carries a range.**
Anything else forces the "restrict trims to subclip boundaries" behaviour to be
special-cased in every edit operation.

### 4.2 Compound / nested sequence — "is a composition referenceable?"

FCPXML: `<!ELEMENT media (multicam | sequence)?>` — a compound clip's construction
is a **resource**, and `ref-clip` *"References a compound clip media"* by IDREF.
MLT: a `tractor` is itself a producer and can be referenced from a playlist entry
like any other. AAF: a CompositionMob referenced by a SourceClip from another
CompositionMob.

OTIO, and therefore `cine-timeline`, does it differently: nesting is **structural
containment**. *"By nesting a `Composition` (either `Track` or `Stack`) we can
refer to a `Composition` as though it was just another `Clip` in the outer
`Composition`"* and *"Nested Stack behaves just like a `Clip` that happens to have
complex contents rather than a simple media reference"*
(<https://opentimelineio.readthedocs.io/en/latest/tutorials/otio-timeline-structure.html>).

The difference is decisive: an inline nested `Stack` used in three places is
**three copies**. Editing one does not change the others. A referenced composition
used in three places is **one thing, three instances** — which is what every
editor expects a compound clip to be. Reference beats containment here, and the
UCG's edge model is well suited to expressing it.

### 4.3 Multicam — "can one instance select among several synchronised materials?"

FCPXML is the clearest public model:

```dtd
<!ELEMENT multicam (mc-angle*, metadata?)>          <!-- a resource, inside <media> -->
<!ELEMENT mc-angle ((%clip_item; | transition)*) >
<!ATTLIST mc-angle angleID CDATA #REQUIRED>
<!ELEMENT mc-clip (note?, %timing-params;, ..., mc-source*, ...)>
<!ATTLIST mc-clip ref IDREF #REQUIRED>              <!-- 'media' ID -->
<!ELEMENT mc-source (audio-role-source*, %intrinsic-params-video;, ...)>
<!ATTLIST mc-source angleID CDATA #REQUIRED>
<!ATTLIST mc-source srcEnable (all | audio | video | none) "all">
```

*"A 'multicam' is a container for multiple "angles" of related content"*; *"An
'mc-angle' is a container for elements ordered serially in time for one angle"*;
*"An 'mc-source' element defines custom settings and filters to apply to an angle
of a multicam clip or edit."* Angles are **spines in their own right** (they can
contain cuts and transitions), the multicam is a shared resource, and each
instance carries `mc-source` entries selecting video from one angle and audio from
another. That is a complete separation of *synchronised group identity* from
*per-instance angle selection*.

Avid's group clips are the counterexample that proves the value: they appear to
have **no publicly documented native AAF representation** and community evidence
says multicam must be committed/flattened before AAF export
(<https://community.avid.com/forums/t/114443.aspx>) — marked **`UNVERIFIED`**, but
the practical lesson is clear: a synchronised group that only exists inside one
application is a group that cannot be handed to anyone else.

The modelling requirement: **a sync group is a node with its own identity, holding
ordered angles with a defined sync origin; an instance selects angles per stream
kind.** Note that CINE-OMNI already computes dual-system audio sync (`cine-timeline`
has two-stage FFT sync) — it produces exactly the offsets a sync group needs, and
currently has nowhere durable to put them.

### 4.4 Time remapping and speed — "is source→timeline time a function?"

The naive model says a clip maps timeline time to source time by a constant
offset. Retiming makes it an arbitrary monotonic-ish function. FCPXML:

```dtd
<!ELEMENT timeMap (timept)*>
<!ATTLIST timeMap frameSampling (floor|nearest-neighbor|frame-blending|optical-flow-classic|optical-flow) "floor">
<!ATTLIST timeMap preservesPitch (0 | 1) "1">
<!ELEMENT timept EMPTY>
<!ATTLIST timept time %time; #REQUIRED>     <!-- new adjusted clip time -->
<!ATTLIST timept value CDATA #REQUIRED>     <!-- original clip time -->
<!ATTLIST timept interp (smooth2 | linear | smooth) "smooth2">
```

*"A 'timeMap' is a container for 'timept' elements that change the output speed of
the clip's local timeline. When present, a 'timeMap' defines a new adjusted time
range for the clip using the first and last 'timept' elements. All other time
values are interpolated."* Apple's own example includes a *negative* segment
(`0s→0s`, `10s→5s`, `20s→0s`) — i.e. play forward then backward. OTIO's
equivalents are `LinearTimeWarp` (*"a time warp that applies a linear speed up or
slow down across the entire clip"*, `time_scalar` where *"2.0 means the clip
occupies half the time in the parent item"*) and `FreezeFrame` (*"Hold the first
frame of the clip for the duration of the clip"*).

Both `%timing-params;` in FCPXML are `(conform-rate?, timeMap?)` — **rate conform
and retime are the same category of thing: a declared transformation between the
material's time and the instance's time.** That is the right abstraction. The
mapping from timeline time to source time must be a **stored, inspectable
function**, not an offset — and reversibility matters, because a viewer scrubbing
backwards from a timeline position needs source time, while a marker placed on
source needs a timeline position.

**`cine-timeline` currently has no effects and no time warps at all.**
`Clip::media_time_at` is `tr.start_time + (track_time - track_start)` — a pure
offset. Adding retiming later without a function abstraction means changing every
call site that computes media time.

---

## 5. Organisation and derived artefacts

### 5.1 Bins and organisation

Two models exist, and the difference matters more than it looks.

* **Hierarchical containment** (folders/bins): an item is in one place. Avid bins,
  Premiere bins, Resolve Media Pool folders, Kdenlive's Project Bin folders.
* **Non-exclusive classification** (keywords/tags/smart collections): an item is
  in as many places as match. FCPXML makes this explicit and first-class:
  `keyword-collection`, `smart-collection`, `collection-folder` are documented
  resource-level concepts alongside `event` and `library`
  (<https://developer.apple.com/documentation/professional-video-applications/fcpxml-reference>),
  and `keyword` is a `%marker_item;` — i.e. **a keyword is a marker with a range**,
  so a keyword can apply to *part* of a clip. OTIO's nearest equivalent is
  `SerializableCollection`: *"A container which can hold an ordered list of any
  serializable objects. Note that this is not a Composition nor is it Composable."*

FCPXML's `smart-collection` goes further and is worth studying as a
specification of *what a project needs to be queryable on*: `match (any | all)`
over `match-clip` (audition / synchronized / compound / multicam / layeredGraphic
/ project), `match-keywords`, `match-markers` (all / standard / allTodo /
complete / incomplete), `match-time`, `match-property`, and —
tellingly — **`match-representation (original | optimized | proxy)`**
(<https://developer.apple.com/documentation/professional-video-applications/smart-collection>).
That last one is only expressible because representation is modelled data: you can
ask "show me everything still running on proxies." In a path-keyed model the
question has no referent.

For a graph-based system this is nearly free: containment is a `Contains` edge,
classification is a `References` edge, and smart collections are stored *queries*.
The trap to avoid is making the bin the owner of the material — an item's identity
must not depend on where it is filed.

### 5.2 Markers, notes, ratings

FCPXML's `%marker_item;` is `(marker | chapter-marker | rating | keyword |
analysis-marker)` — five kinds of *ranged annotation* sharing one slot. `marker`
is *"An annotation, with an optional to-do flag, that applies a text marker to a
range of time"*, and *"If 'completed' is specified, this marker becomes a to-do
item"* (DTD line 665). `note` is a separate child element on clips.

The generalisable concepts: an annotation has (a) an owner, (b) a **range**, not a
point, (c) a coordinate system — source time or timeline time — and (d) a kind
that determines behaviour. OTIO's `Marker` has `marked_range` *"relative to the
Item this marker is attached to"*, which encodes (c) implicitly through ownership.

Where markers *live* is the question that bites: a marker on a material must
follow the material into every instance; a marker on an instance must not. Both
are legitimate and both are needed, so the coordinate system has to be explicit.
`cine-timeline` already gets this right — `Clip.markers` are documented as being
"in the clip's media time coordinates" while `Track`/`Stack` markers are in
composition time.

### 5.3 Versioning, renders and cache

Three distinct kinds of derived artefact, routinely conflated:

| Kind | Input | Invalidated by | Deletable |
|---|---|---|---|
| **Version** (v001, v002, an audition alternative) | authored | nothing — versions are history | no |
| **Render / optimised media** | a defined operation over defined inputs | any input change | yes, regeneratable |
| **Cache** | the same, but purely performance | anything | yes, silently |

FCPXML's `audition` is the version case done well:
`<!ELEMENT audition (audio | video | title | ref-clip | asset-clip | clip | sync-clip)+>`,
described as *"Contains one active story element followed by alternative story
elements in the audition container"* — the first child is the active take and the
rest are alternates, kept *in place*, structurally, not in a side file. That is worth
copying conceptually: a version is a sibling, not a snapshot.

For renders and cache the requirement is the one CINE-OMNI's reactor architecture
is actually built for: **a derived artefact must record what it was derived from,
so that a change to the input marks it stale automatically.** The UCG already has
`DerivedFrom` (which propagates dirtiness) and `attrs::STALE`. What is missing is
any node kind for a derived *media* artefact to attach to.

---

## 6. What OTIO does and does not cover

This section is deliberately precise, because "we are OTIO-compatible" is easy to
over-read.

### 6.1 OTIO's own statement of scope

> *"OpenTimelineIO is an interchange format and API for editorial cut
> information."*
>
> *"OTIO contains information about the order and length of cuts and references to
> external media. **It is not however, a container format for media.**"*

— <https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/main/README.md>

Two further self-limitations, quoted from the docs:

> *"If there are effects on a clip, OpenTimelineIO does not say anything about the
> impact of the effect."*
>
> *"a clip's source_range could refer to a segment outside the available_range of
> its media reference. That is fine, and comes up in practice often."* … *"OTIO
> itself does no snapping or verification."*

— <https://opentimelineio.readthedocs.io/en/latest/tutorials/otio-timeline-structure.html>

And on media resolution, the MediaLinker plugin documentation:

> *"OpenTimelineIO Media Linkers are plugins that allow OTIO to replace
> MissingReferences with valid, site specific MediaReferences after an adapter
> reads a file."*

— <https://opentimelineio.readthedocs.io/en/latest/tutorials/write-a-media-linker.html>

That is OTIO explicitly declaring media resolution to be **the application's job,
delegated to a site-specific plugin**. The gap is not an oversight; it is the
design.

Finally, on fidelity: `.otio` is the only lossless format; *"Other formats are
lossy - they will only store and retrieve features that are supported by that
format (and by the adapter implementation)"*
(<https://opentimelineio.readthedocs.io/en/latest/tutorials/adapters.html>).

### 6.2 The honest ledger

| Concern | OTIO | What a working NLE additionally needs |
|---|---|---|
| Composition structure (tracks, stacks, clips, gaps, transitions, markers) | **covered, well** | — |
| Trims and ranges | **covered** | — |
| Time algebra | **covered in shape** (`RationalTime`/`TimeRange`) but float-typed | exact rational arithmetic; explicit rounding policy |
| Multiple representations per clip | **covered** (`media_references()` + `active_media_reference_key`) | material-level identity above it |
| Material identity | **not covered** — no `uid`, no UMID | persistent, deterministic material IDs |
| Physical source / reel / capture event | metadata only | first-class, for conform |
| Container / stream description | **not covered** | codec, dimensions, pixel format, colour, channel layout, sample rate, stream indices |
| Decode | **explicitly out of scope** | a frame source: seek, decode, GOP handling, hardware paths |
| Frame cache / memory budget | not covered | required for playback at all |
| Conform state | not covered | offline/online/mismatched, and *which key matched* |
| Render / cache state | not covered | derived artefacts with invalidation |
| Effect semantics | **explicitly not covered** (*"does not say anything about the impact"*) | actual evaluation |
| Referenced (shared) compositions | not covered — nesting is inline containment | compound clips as referenced identity |
| Sync groups / multicam | not covered | first-class |
| Validation | **explicitly not performed** | invariants, but see §9.2 — do not over-enforce |
| Organisation (bins/keywords) | `SerializableCollection` only | folders, tags, saved queries |
| Concurrency, undo, provenance | not covered | the UCG already provides these |

**Reading:** OTIO is the right *interchange* target and roughly the right
*composition* model. It is not, and does not claim to be, a media model. Treating
OTIO conformance as sufficient for Phase 2 would be a category error.

---

## 7. The distilled conceptual model

This is the minimum. Fewer entities than this and something in §1–§6 becomes
unrepresentable; more and you are modelling a specific product rather than the
domain.

```
                    ┌──────────────────┐
                    │ PhysicalSource   │  card / reel / roll / tape / camera magazine
                    └────────┬─────────┘
                             │ produced
                    ┌────────▼─────────┐
                    │ CaptureEvent     │  one recording: shoot day, camera, take
                    └────────┬─────────┘
                             │ recorded
   ╔═════════════════════════▼══════════════════════════════════╗
   ║ MATERIAL   the content, invariant under copy/transcode/move ║  ← the keystone
   ║   • material identity (stable, never derived from a path)   ║
   ║   • native timing: exact rate, duration, VFR flag           ║
   ║   • timecode track: origin + drop/non-drop + rate           ║
   ║   • stream inventory: what video/audio it has               ║
   ╚══╤════════════════════════════════════╤════════════════════╝
      │ has 1..n                           │ has 0..n
   ┌──▼───────────────┐               ┌────▼──────────────┐
   │ Representation   │               │ Annotation        │ marker/keyword/note,
   │  • role: original│               │  ranged, in       │ in SOURCE time
   │    /proxy/opt/   │               │  source time      │
   │    render        │               └───────────────────┘
   │  • locator (URI) │
   │  • verification  │  ← the ONLY place a file path is allowed to live
   │  • resolvability │
   └──┬───────────────┘
      │ contains 1..n
   ┌──▼───────────────┐
   │ Stream           │ codec, dimensions, pixel/sample format, colour,
   │                  │ channels, exact rate, frame/sample count
   └──────────────────┘

MATERIAL-LIKE things — anything an edit may reference. Same interface: identity,
a time domain, an available range, streams (real or computed):

   Material  │  Subclip (DerivedFrom Material, + range)
             │  Composite (a composition, referenced not inlined)
             │  SyncGroup (ordered Angles, each Material + sync offset)

   ┌───────────────────────────────────────────────────────────────┐
   │ INSTANCE — one placement on a timeline                        │
   │   references exactly one Material-like thing                  │
   │   + source range (which part)                                 │
   │   + a TimeTransform: source time ⟷ instance time              │
   │       (identity | rate conform | speed | time map | freeze)   │
   │   + per-instance stream selection (which angle, which channels)│
   │   + annotations in INSTANCE time                              │
   └───────────────────────────────────────────────────────────────┘
                             │ placed in
   ┌─────────────────────────▼─────────────────────────────────────┐
   │ COMPOSITION — timeline / stack / track (already implemented)   │
   └───────────────────────────────────────────────────────────────┘

CROSS-CUTTING:
   ConformState   per Material and per Instance: online/offline/unresolved/
                  substituted/mismatched, + WHICH match key succeeded
   Derivation     any Representation the system produced: what from, by what
                  operation, with what parameters — so it can be invalidated
   Organisation   bins (exclusive containment) and tags/queries (non-exclusive)
```

**The six load-bearing rules.**

1. **Material identity is never a path.** A path is a property of a
   Representation. This is rule zero and everything else depends on it.
2. **The edit references the Material, never the Representation.** Choosing a
   representation is a resolution step performed at playback, not an edit.
3. **Representations are plural and typed by role.** One material, many encodings,
   one resolution policy.
4. **Every reference is (identity, stream/slot, range)** — AAF's
   `sourceID` / `sourceTrackID` / `startTime`, restated.
5. **Source→instance time is a stored function, not an offset.** Rate conform and
   speed change are the same kind of thing.
6. **Derived artefacts declare their inputs.** Otherwise nothing can be
   invalidated correctly and the cache lies.

---

## 8. Proposed UCG mapping

### 8.1 The rule for node vs attribute vs edge

A concept becomes a **node** if any of these holds; otherwise it is an attribute:

* it can be referenced from more than one place;
* it needs its own annotations, metadata or provenance;
* it has an independent lifetime (can outlive or predecease its neighbours);
* it must be independently dirtied, derived or invalidated.

A concept becomes an **edge** if it is a *relationship whose direction and
propagation semantics matter*. Note the hard constraint: **`ucg_core::Edge` is
`{from, to, kind}` with no attributes.** Anything that would want to be an edge
property (a sync offset, a match key, a channel mapping) has to become a node
attribute on one endpoint, or a small intermediary node. This is a real
limitation and it shapes several choices below.

### 8.2 Proposed node kinds (`ucg_schema::kinds`)

| Kind | Represents | Deterministic id | Key attributes |
|---|---|---|---|
| `media.source` | card / reel / roll / tape / camera magazine | `from_key("source:<reel-or-card-key>")` | `reel`, `card_id`, `camera`, `roll`, `shoot_date`, `medium` (card/tape/film) |
| **`media.material`** | **the content — the keystone** | see §8.4 | `identity_keys` (list of typed keys), `identity_key_kind`, `name`, `has_video`, `has_audio`, `native_rate` (exact rational), `native_duration`, `is_vfr`, `timecode` (component: origin + drop + rate), `available_range` |
| `media.representation` | one encoding of a material | `derived(material, "rep:<role>:<locator-or-hash>")` | `role` (`original`\|`proxy`\|`optimised`\|`render`\|`transcode`), `locator` (URI), `content_hash`, `size`, `mtime`, `verified_at`, `resolvable`, `available_range` |
| `media.stream` | one elementary stream | `derived(representation, "stream:<index>")` | `stream_kind` (video/audio/data/timecode), `index`, `codec`, `width`, `height`, `pixel_format`, `bit_depth`, `colorspace`/`transfer`/`primaries` (reuse `ColorMeta`), `sample_rate`, `channels`, `channel_layout`, `rate` (exact rational), `frame_count` |
| `media.subclip` | named bounded view of a material | `derived(material, "subclip:<name>")` | `range`, `name`, `restrict_trims` |
| `media.composite` | a compound clip as *referenced* identity | `from_key("composite:<key>")` | `composite_kind` = `compound`, `available_range` |
| `media.sync_group` | multicam / dual-system group | `from_key("syncgroup:<key>")` | `sync_origin`, `angle_count` |
| `media.angle` | one angle of a sync group | `derived(sync_group, "angle:<angle_id>")` | `angle_id`, `sync_offset` (RationalTime), `order` |
| `media.bin` | organisational folder | `derived(parent, "bin:<name>")` | `name`, `order` |
| `media.query` | smart bin / saved query | `from_key("query:<name>")` | `expression` |

Existing `asset.media` becomes an alias/migration target for `media.material`
(§9.3). Existing `nle.*` kinds are unchanged.

### 8.3 Proposed edges (no new `EdgeKind` required)

The six existing edge kinds are sufficient. That is a good sign for the model.

| From → To | Edge | Why this kind |
|---|---|---|
| `project` → `media.bin` → `media.material` | `Contains` | exclusive organisation, ownership lifecycle |
| `media.material` → `media.representation` | `Contains` | a representation cannot outlive its material |
| `media.representation` → `media.stream` | `Contains` | same |
| `media.material` → `media.source` | `DerivedFrom` | provenance **and** correct propagation: fixing a reel name on the card *should* re-evaluate conform on everything shot on it |
| `media.subclip` → `media.material` | `DerivedFrom` | if the material's available range changes, the subclip must be re-validated |
| `media.representation`(proxy/optimised/render) → `media.representation`(original) | `DerivedFrom` | **this is the invalidation edge**: regenerate when the source changes. A *user-supplied* proxy gets no such edge — that distinction is exactly what says whether the system may silently regenerate it |
| `media.composite` → `nle.stack` | `Contains` | the composite owns its composition |
| `media.sync_group` → `media.angle` | `Contains` | ordered ownership |
| `media.angle` → `media.material` | `References` | an angle points at material it does not own; the sync offset lives on the angle node (edges carry no attributes) |
| **`nle.clip` → `media.{material\|subclip\|composite\|sync_group}`** | **`Binds`** | **the edit reference — exactly one per clip** |
| `nle.clip` → `media.stream` (per selected stream) | `References` | angle/channel selection, when an instance needs it |
| `media.representation` → `color.*` | `References` | colour metadata resolution |
| `nle.clip` → `script.scene` | `SyncedTo` | unchanged, existing behaviour |

### 8.4 The identity question, answered honestly

The tempting design is `EntityId::from_key("material:<UMID>")`. It is wrong, and
the reason is important: **you learn better identifiers over time.** A material
first seen as a loose file has only a path; after a UMID is read from its MXF, or
a hash is computed, or an AAF import supplies a MobID, the *same content* has a
stronger key. If the id is a function of the key, learning the key changes the id
— and every `Binds` edge from every clip points at a node that no longer exists.

The workable design has three parts:

1. **Mint the material id once**, deterministically, from the strongest key
   available *at first sight*, namespaced by key kind so the provenance of the id
   is legible:
   `EntityId::from_key("material:umid:<umid>")`, else
   `material:fcp-uid:<uid>`, else `material:sha256:<hash>`, else
   `material:path:<canonical url>`. Re-importing the same document is then
   idempotent, which the codebase requires.
2. **Store every key you ever learn** as `identity_keys` on the material, and
   maintain a key→material index. Resolution consults the index; it never
   recomputes the id.
3. **Make merge a first-class operation.** When two materials are discovered under
   different keys and later proven identical, `merge(a, b)` retargets every
   `Binds` edge and unions the key sets. Identity merging is unavoidable in this
   domain — the choice is whether it is an explicit, tested, undoable transaction
   or an emergent bug.

### 8.5 The proxy switch, as a worked example of the invariant

§3.4 says swapping representations must not touch an edit decision. In this
mapping that becomes a testable property:

* a project-level `playback_role` attribute (`original` | `proxy` | `auto`) on the
  `project` node, with an optional per-material override;
* switching the whole project to proxies is **one attribute set on one node**;
* resolution walks `nle.clip --Binds--> media.material --Contains--> media.representation`
  and picks by role, falling back when unresolvable;
* `EdgeKind::Binds` **does not propagate dirtiness** (`EdgeKind::propagates()`
  returns false for it) — which is exactly right here, and is an argument for
  leaving it that way. Media changing must not dirty edits.
* Conform state is therefore *pushed* by a reactor that watches material and
  representation nodes and sets `needs_conform` on bound clips — the mechanism
  `cine-pipeline`'s conform reactor already implements.

**The acceptance test writes itself:** toggle `playback_role`, re-run the
reactors, and assert that no `nle.*` node appears in `group.deltas`. If it does,
the model has leaked the representation into the edit.

This is not a speculative design: Resolve ships exactly this control as a
three-state project-level *Proxy Handling* menu — *"Prefer Camera Originals"*,
*"Prefer Proxies"*, *"Disable All Proxies"* — and the documented consequence of
the third state is that clips *"display as offline"* rather than break
(<https://documents.blackmagicdesign.com/UserManuals/DaVinci-Resolve-20-Editors-Guide.pdf>).
One attribute, project-wide, edits untouched, and offline as a first-class
resolution state.

### 8.6 What deliberately stays an attribute

| Concept | Why not a node |
|---|---|
| Timecode origin + DF/NDF + rate | a property of exactly one owner; never shared, never independently dirtied. A `Timecode` component on `media.material` / `media.stream` / `nle.timeline` — **not** a field on `RationalTime` |
| Exact frame rate `(num, den)` | a value, not an entity |
| TimeMap / conform-rate / speed | belongs to exactly one instance; a `TimeTransform` component on `nle.clip` |
| Conform state and match key | per-node state, and edges cannot carry attributes anyway |
| Colour metadata | already a `ColorMeta` component; keep it, attach to `media.stream` |
| Frame cache | **not in the graph at all.** Cache keys should be *derived* from graph node ids plus the attributes that affect pixels, so the graph stays authoritative and the cache stays disposable. Putting cache entries in the UCG would make an ephemeral, machine-local, size-bounded thing part of the document |

---

## 9. `cine-timeline` assessed against this model

### 9.1 What it already gets right — and it is more than it looks

| # | Strength | Where |
|---|---|---|
| 1 | **The composition model is genuinely correct, not approximately correct.** Transitions take no track time and overlap the cut; `range_of_child` reproduces OTIO's `range_of_child_at_index` including the transition case; stacks compose in parallel, tracks sequentially | `otio.rs` `Track::range_of_child`, `Stack::duration` |
| 2 | **The hard part is done.** `ripple_trim`, `roll`, `slip`, `slide`, `insert`, `overwrite`, `lift`, `extract`, `split_at`, `join`, `diff` are the edit operations that take longest to get right, and they exist with 61 tests | `edit.rs` |
| 3 | **Time is already *shaped* as a rational** (`value` / `rate`), not as seconds. The shape is right; only the type is wrong. That makes §2.3's fix a type change, not a redesign | `ucg-core/src/time.rs` |
| 4 | **Media is already a separate node bound by an edge**, not a string on the clip: `nle.clip --Binds--> asset.media`, with exactly one such edge enforced | `graph.rs::write_media_reference` |
| 5 | **`ucg_id` round-trips through OTIO metadata**, so entity identity survives a trip through external OTIO tools. This is rare and genuinely valuable | `otio.rs` `UCG_ID_KEY` |
| 6 | **Graph import is idempotent** — re-importing an unchanged timeline produces zero ops, which is the precondition for reactor convergence | `graph.rs::import_timeline` |
| 7 | **Markers already carry the right coordinate systems** — clip markers in media time, track/stack markers in composition time | `otio.rs` |
| 8 | **`needs_conform` already exists as first-class state** with reactor support and tests | `ucg-schema`, `cine-pipeline/src/reactors.rs` |
| 9 | **Missing and Generator references are modelled explicitly**, so "no media" is a state rather than a null | `MediaReference::Missing` / `::Generator` |

The composition layer needs no rework. Everything below it needs building.

### 9.2 What it lacks, precisely

Ordered by how expensive each becomes if deferred.

| # | Gap | Evidence in the code | Cost of deferring |
|---|---|---|---|
| **1** | **Media identity is the file path.** `EntityId::from_key(&format!("media:{target_url}"))` | `graph.rs::write_media_reference` | **Highest.** Rename a file and you get a *different entity*; every `Binds` edge, every grade, every marker, every `SyncedTo` binding is orphaned. Two copies of the same material at two paths are two unrelated entities. Changing an identity function after data exists is the most expensive migration a graph system can face |
| **2** | **One representation per media.** `MediaReference` is an enum with a single `External { target_url, available_range }`. Worse: `otio_json.rs` *reads* `Clip.2`'s `media_references` map, takes only `active_media_reference_key`, and **discards every other reference** — a proxy-bearing OTIO file loses its proxy silently on import and never writes it back | `otio.rs`, `otio_json.rs` lines 237–248 | Proxies, optimised media and offline/online are unrepresentable. This is also a **live OTIO interop data-loss bug**, independent of Phase 2 |
| **3** | **No stream or container description whatsoever.** `asset.media` carries `media_path`, `name`, `available_range` — no codec, dimensions, pixel format, bit depth, channel count, sample rate, stream index | `graph.rs`, `ucg-schema/attrs` | The decoder has nothing to negotiate with and the viewer has nothing to configure from |
| **4** | **Time is `f64`.** Measured: **13.2 % of 23.976 frame indices floor to the wrong frame** after one rescale through a 24 fps track; 14.9 % through a two-level nest; frame 1 → frame 0. Three `1e-9` epsilons in `Track::validate` and an `almost_equal(eps_seconds)` are the workaround | `ucg-core/src/time.rs`, §2.3 | Frame-accuracy is not achievable. Every consumer built on it inherits the defect |
| **5** | **No drop-frame anywhere.** `to_timecode()` uses `self.rate.round()`; `from_timecode` accepts `;` but treats it as `:`. Measured: a one-hour 29.97 programme is mislabelled by **108 frames (3.60 s)** | `ucg-core/src/time.rs`, §2.4 | Broadcast deliverables are wrong. Fix is a `Timecode` concept, not a flag on `RationalTime` |
| **6** | **No timecode origin except `Timeline::global_start_time`.** Materials have no `tcStart`; there is no DF/NDF anywhere | `otio.rs` | Timecode+reel conform (§3.2) is impossible — the second-strongest match key is unavailable |
| **7** | **No effects and no time warps at all.** `read_otio` explicitly errors on `Effect.1`. `Clip::media_time_at` is `tr.start_time + (track_time - track_start)` — a pure offset | `otio_json.rs:507`, `otio.rs` | Retimes, speed changes and rate conform have nowhere to live, and adding the function abstraction later touches every media-time call site |
| **8** | **No `ImageSequenceReference`.** DPX/EXR frame sequences are the native form of most finishing and VFX media | `MediaReference` enum | An entire class of professional media cannot be referenced |
| **9** | **Nesting is inline containment, not reference.** `Composable::Stack` embeds a stack; the same compound used three times is three independent copies | `otio.rs` | Compound clips do not behave the way editors expect. Inherited from OTIO, but OTIO is an interchange format and CINE-OMNI is not |
| **10** | **No sync group or multicam.** The FFT dual-system sync in `sync.rs` computes exactly the offsets a sync group needs and has nowhere durable to put them | `sync.rs` | The most valuable thing the crate already computes is thrown away after use |
| **11** | **`Track::validate()` is stricter than OTIO and will reject valid OTIO documents.** It errors when `source_range` exceeds `available_range`; OTIO states this *"is fine, and comes up in practice often"* and that *"OTIO itself does no snapping or verification"* | `otio.rs::Track::validate` | A conformance bug in a crate whose value proposition is OTIO conformance. The right answer is to keep the check but downgrade it from an error to a **`mismatched` conform state** (§3.1) |
| **12** | **A track has no declared rate.** `Track::rate()` returns the rate of the first timed child, falling back to `24.0`; `Stack::duration()` compares children via `to_seconds()` floats | `otio.rs` | Rate is inferred from content instead of declared, so mixed-rate tracks silently pick an arbitrary reference rate — and then §2.3's rescale error applies to everything on them |
| **13** | **Conform state is one boolean.** `needs_conform: bool` records that something is wrong, never *what* or *which match key was used* | `ucg-schema/attrs` | Relink cannot be trusted or audited; "relinked by filename" is indistinguishable from "relinked by UMID" |
| **14** | **No node kind for derived media.** Renders, optimised media and background exports have nowhere to attach, so nothing can be invalidated | `ucg-schema/kinds` | The reactor architecture's main strength goes unused for exactly the artefacts that most need it |
| **15** | **No organisation layer.** Clips bind straight to `asset.media`; there are no bins, folders, keywords or saved queries | `graph.rs` | Unusable above a few dozen clips |

### 9.3 The migration this implies

Gap 1 is the one that must be fixed before real media exists, not after — an
identity function is the hardest thing in a graph to change once edges depend on
it. Concretely:

1. `asset.media` keeps its kind but is **reinterpreted as `media.material`**: the
   node stays, `media_path` moves down to a new `media.representation` child with
   `role = original`, and the material gains `identity_keys`.
2. Ids minted by `from_key("media:<url>")` become `material:path:<url>` keys in
   `identity_keys`, and the node id is preserved — so existing graphs keep every
   `Binds` edge. The path-derived id is grandfathered, not perpetuated.
3. New materials mint ids per §8.4, preferring UMID > vendor uid > content hash >
   path.
4. `MediaReference` gains a keyed-map form matching OTIO `Clip.2` so gap 2's data
   loss stops, and the map maps onto `Contains`-ed representations.

Steps 1–2 are a one-transaction migration with no external dependency and no
decoder. **They can and should be done before the media engine exists**, because
their cost grows with every graph written under the old identity.

---

## 10. Register of what could not be verified

Recorded per the blueprint's standing rules rather than filled in plausibly.

| Claim | Status | What was tried |
|---|---|---|
| AAF Object Specification verbatim prose and section numbers | **`UNVERIFIED`** | `static.amwa.tv/ms-01-aaf-object-spec.pdf` and the SourceForge mirror both fetched; PDF text extraction failed (compressed streams). Object model established instead from the AAF COM API reference docs and `pyaaf2`, both generated from the same model |
| AAF Edit Protocol's required conform field list | **`UNVERIFIED`** | `static.amwa.tv/as-01-aaf-edit-protocol-spec.pdf` — only the table of contents was machine-readable |
| SMPTE ST 330 numeric material-number generation-method table | **`UNVERIFIED`** | Standard is paywalled. Material/instance-number *semantics* are verified verbatim from `pyaaf2`'s ST 330 implementation docs |
| SMPTE ST 12-1 exact normative drop-frame wording | **`UNVERIFIED`** | Paywalled. The rule and residual error are corroborated across SMPTE's own blog and secondary sources; the arithmetic was independently verified by computation (§2.4) |
| Avid `.mdb` / `.pmr` binary format | **`UNVERIFIED` / not publicly specified** | Only an Avid KB troubleshooting article exists, not a format spec |
| Avid multicam group clips in AAF | **`UNVERIFIED` / likely unsupported** | Only community-forum evidence that multicam must be committed before export |
| Avid "Use tape sources when present" exact semantics | **`UNVERIFIED`** | Vendor-forum evidence only |
| Whether FCPXML `media-rep/@sig` is a content hash | **`UNVERIFIED`** | Apple documents it only as *"The unique identifier of a media representation, assigned by Final Cut Pro"* |
| MOV/MP4 persistent material-UUID support in practice | **`UNVERIFIED`** | Not investigated to primary sources |
| Premiere Match File Properties checkbox set beyond `File Extension` | **`UNVERIFIED`** | `helpx.adobe.com` mostly unreachable from this environment (TLS hangs via both the fetch tool and `curl`); a reader-proxy path got through for some pages and confirmed `File Extension`, subclips, multicam, and nested sequences verbatim (§1.6), but the full checkbox list (`File Name`/`Media Start`/`Tape Name`) remains search-snippet-only |
| Premiere "Attach Proxies" / "Toggle Proxies" pages, merged clips page | **`UNVERIFIED`** | Guessed URLs 404'd; not located |
| Premiere "Restrict Trims to Subclip Boundaries" | **`UNVERIFIED`** | Could not be confirmed on any Adobe page; the term may not exist |
| Resolve "Reconform from Bins" as distinct terminology | **`UNVERIFIED`** | The guides fetched use *"Relink Clips for Selected Bins"*; the full Reference Manual PDF exceeded the fetch size limit |
| Kdenlive `kdenlive:proxy` semantics, clip ids, offline-clip state | **`UNVERIFIED` from documentation** | Manual pages fetched; these live in source (`invent.kde.org/multimedia/kdenlive`), which is GPL-3.0 and therefore study-only |
| `mltframework.org/docs/mltxml/` prose | **fetch failed** (TLS certificate error) | Substituted the DTD from the MLT repository, which is stronger evidence anyway |

---

## 11. Sources

**Specifications and standards**
- AAF Object Specification — <https://static.amwa.tv/ms-01-aaf-object-spec.pdf> (mirror <https://aaf.sourceforge.net/docs/aafObjectModel.pdf>)
- AAF Edit Protocol — <https://static.amwa.tv/as-01-aaf-edit-protocol-spec.pdf>
- AAF COM API reference — <https://aaf.sourceforge.net/docs/aafapiman/> (`IAAFSourceClip`, `IAAFMasterMob`, `IAAFSourceMob`, `IAAFCompositionMob2`, `IAAFTimecode`)
- SMPTE ST 330 (UMID) — <https://standards.globalspec.com/std/1396751/smpte-st-330>; registration authority <https://www.smpte-ra.org/umid-information>
- SMPTE ST 12-1 (time code) — <https://standards.globalspec.com/std/1674717/smpte-st-12-1>; <https://www.smpte.org/blog/understanding-standards-time-code>
- ISO/IEC 14496-12 (ISO base media file format, `stts`) — <https://b.goeswhere.com/ISO_IEC_14496-12_2015.pdf>; <https://www.iso.org/obp/ui/#iso:std:iso-iec:14496:-12:ed-4:en>
- CMX 3600 EDL — <https://www.edlmax.com/edlmaxhelp/Edl/maxguide.html>; <https://en.wikipedia.org/wiki/Edit_decision_list>
- SMPTE timecode overview — <https://en.wikipedia.org/wiki/SMPTE_timecode>

**Apple / FCPXML**
- FCPXML Reference — <https://developer.apple.com/documentation/professional-video-applications/fcpxml-reference>
- `asset` — <https://developer.apple.com/documentation/professional-video-applications/asset>
- `media-rep` — <https://developer.apple.com/documentation/professional-video-applications/media-rep>
- `media` — <https://developer.apple.com/documentation/professional-video-applications/media>
- `format` — <https://developer.apple.com/documentation/professional-video-applications/format>
- Story Elements — <https://developer.apple.com/documentation/professional-video-applications/story-elements>
- `conform-rate` — <https://developer.apple.com/documentation/professional-video-applications/conform-rate>
- Timing Attributes — <https://developer.apple.com/documentation/professional-video-applications/timing-attributes>
- `timeMap` — <https://developer.apple.com/documentation/professional-video-applications/timemap>
- Multicam Media — <https://developer.apple.com/documentation/professional-video-applications/multicam-media>
- `smart-collection` / `keyword-collection` / `marker` — <https://developer.apple.com/documentation/professional-video-applications/smart-collection>
- Document Type Definition (in-page FCPXML DTD) — <https://developer.apple.com/documentation/professional-video-applications/document-type-definition>
- FCPXML DTDs v1.8 / v1.9 / v1.10 / v1.11 (Apple's DTDs, mirrored in the CommandPost repository) — <https://github.com/CommandPost/CommandPost/tree/develop/src/extensions/cp/apple/fcpxml/dtd>

**OpenTimelineIO**
- README (scope statement) — <https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/main/README.md>
- Timeline structure — <https://opentimelineio.readthedocs.io/en/latest/tutorials/otio-timeline-structure.html>
- Schema API — <https://opentimelineio.readthedocs.io/en/latest/api/python/opentimelineio.schema.html>
- `opentime` API — <https://opentimelineio.readthedocs.io/en/latest/api/python/opentimelineio.opentime.html>
- Media linkers — <https://opentimelineio.readthedocs.io/en/latest/tutorials/write-a-media-linker.html>
- Adapters — <https://opentimelineio.readthedocs.io/en/latest/tutorials/adapters.html>
- CMX3600 adapter — <https://github.com/OpenTimelineIO/otio-cmx3600-adapter>

**Open-source implementations studied (not linked, per the licensing spine)**
- `pyaaf2` — <https://github.com/markreidvfx/pyaaf2>, docs <https://pyaaf.readthedocs.io/> (`aaf2.mobs`, `aaf2.mobid`, `aaf2.essence`)
- LibAAF — <https://github.com/agfline/LibAAF>
- AAF SDK reference implementation — <https://github.com/dneg/aaf>
- MLT — <https://github.com/mltframework/mlt> (DTD: `src/modules/xml/mlt-xml.dtd`; licence: `COPYING`, LGPL-2.1 framework / GPL tools); <https://www.mltframework.org/docs/profiles/>, <https://mltframework.org/docs/mlt++/>, <https://www.mltframework.org/docs/copyrightpolicy/>
- Kdenlive — <https://github.com/KDE/kdenlive> (GPL-3.0, `COPYING`); <https://docs.kdenlive.org/>

**Vendor and practitioner**
- The Editor's Guide to DaVinci Resolve 20 (Blackmagic) — <https://documents.blackmagicdesign.com/UserManuals/DaVinci-Resolve-20-Editors-Guide.pdf>
- The Beginner's Guide to DaVinci Resolve 16 (Blackmagic) — <https://documents.blackmagicdesign.com/UserManuals/DaVinci-Resolve-16-Beginners-Guide.pdf>
- Premiere Pro subclips — <https://helpx.adobe.com/premiere/desktop/edit-projects/intro-to-editing/create-a-subclip-from-the-timeline.html>, <https://helpx.adobe.com/premiere/desktop/edit-projects/intro-to-editing/convert-subclip-to-master-clip.html>
- Premiere Pro multicam source sequences — <https://helpx.adobe.com/premiere/desktop/edit-projects/set-up-multi-camera-sequences-for-editing/create-a-multi-camera-source-sequence.html>
- Premiere Pro nested sequences — <https://helpx.adobe.com/premiere/desktop/edit-projects/edit-nested-sequences/about-nested-sequences.html>
- Premiere Pro automatic relink — <https://helpx.adobe.com/premiere/desktop/organize-media/file-organization/relink-offline-media-automatically.html>
- Premiere Pro proxy workflow, merged clips, and the full Match File Properties checkbox set — `helpx.adobe.com` (**mostly unreachable from this environment; these specific claims remain `UNVERIFIED`, see §10**)
- Avid media database KB — <https://kb.avid.com/pkb/articles/en_US/troubleshooting/en262645>
- Avid AAF tape-source discussion — <https://duc.avid.com/showthread.php?t=355581>
- Avid multicam / AAF discussion — <https://community.avid.com/forums/t/114443.aspx>
- Resolve↔Avid round trip (third-party) — <https://help.frameone.com/support/solutions/articles/69000800937-the-resolve-to-avid-round-trip>

**CINE-OMNI sources read**
- `crates/cine-timeline/src/otio.rs`, `otio_json.rs`, `graph.rs`, `edit.rs`, `sync.rs`
- `crates/ucg-core/src/time.rs`, `id.rs`, `node.rs`
- `crates/ucg-schema/src/kinds.rs`, `lib.rs`, `components.rs`
- `crates/cine-pipeline/src/reactors.rs`, `project.rs`
