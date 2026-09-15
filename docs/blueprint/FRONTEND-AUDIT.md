# Frontend audit — what the backend can do, and what you can see

Written before the application-shell work, from inspection of the running
system rather than from the documentation.

**The finding in one line:** the backend has 16 crates of verified capability
and the browser shows scenes, a text report and a change log. Almost everything
built in the last several phases is invisible.

---

## A. What the backend can already do

| Area | State | Where |
|---|---|---|
| UCG graph, 60 node kinds, reactive cascade, op log, undo/redo, save | Working | `ucg-core`, `ucg-schema` |
| One runtime, one engine, many clients; commits broadcast to all | Working | `cine-runtime::session`, `server::spawn_change_broadcaster` |
| HTTP static serving + `/health` + WebSocket upgrade, path-traversal refusal | Working | `cine-runtime::http` |
| JSON-RPC over WebSocket **and** stdio, one dispatch | Working | `cine-runtime::rpc` |
| Instance lock, stale reclamation, crash recovery from the op log | Working | `cine-runtime::lock`, `cine-cli::start` |
| Screenplay import (Fountain/FDX), scenes, dialogue, breakdown, schedule | Working | `cine-script`, `cine-logistics` |
| OTIO timeline model, editing operations, session state, graph import/export | Working | `cine-timeline` |
| Media identity, probe, H.264 decode, exact time, frame cache | Working | `cine-media` |
| Decode-ahead, playback scheduler, transport, drop accounting | Working | `cine-media::{schedule, decode_ahead}` |
| Native workspace, real playback at 24 fps | Working | `cine-shell` |
| Capability registry with evidence, refusing hardware claims | Working | `cine-media::capability`, `cine-shell::ShellCapabilities` |

## B. What the frontend currently displays

`ui/src` is 5 files. `App.tsx` is 96 lines. It shows:

* connection state and the engine revision
* a list of scenes (`scenes` RPC)
* a scene report as preformatted text (`scene_report`)
* a cascade log of `changed` notifications
* an inspector showing the selected scene's fields

That is the whole of it.

## C. Backend functionality that is invisible in the browser

* **The entire editorial layer.** No timeline, tracks, clips, ruler, playhead,
  selection, track state, links, snapping — none of it reaches the frontend.
* **All media.** No media list, no identity, no metadata, no decoded picture.
* **The capability model.** The browser makes no claims at all, honest or
  otherwise, about what works.
* **Playback.** Real playback exists in `cine-shell`, a separate native binary.
  The browser has no transport and no frame.
* **Project structure beyond scenes.** Shots, takes, previs cameras, audio
  sessions, comp graphs and their relationships are in the graph and unseen.
* **Undo/redo.** In the RPC, absent from the interface.

## D. Existing protocol that can already expose some of it

`stats`, `node`, `kind` are generic graph access and can carry structure without
new methods. `set` mutates any attribute through a real transaction. `undo`,
`redo`, `save`, `validate` exist. The `changed` broadcast already carries
revision, label, touched ids and dirty count to every client.

So **live synchronisation does not need a new architecture** — it needs to be
used.

## E. Protocol that must be added

| Method | Why an existing one will not do |
|---|---|
| `capabilities` | The capability model is Rust-side only; the browser cannot state honestly what works without it |
| `project` | Project identity, path, revision, counts — currently only reachable as three separate calls |
| `timelines`, `timeline` | A timeline is a tree of stacks, tracks, clips and ranges. Walking it through `node` would be dozens of round trips and would put the composition maths in the browser, which is a second source of truth |
| `media`, `media_info` | Same reason, plus stream and codec detail |
| `structure` | The screenplay → scene → shot → take → media chain is edges, not nodes; the browser should not re-derive it |
| `track_state` | Lock/mute/solo/target live in OTIO metadata on a track node; setting them needs the same typed path `cine-timeline` reads |
| `ingest`, `import_otio` | Getting real media and a real cut into a project is currently only possible from the CLI |
| `GET /frame` | A decoded picture has to reach the browser as an image; JSON cannot carry it |

## F. What must be shown as unavailable or degraded, and why

* **Real-time playback in the browser: UNAVAILABLE.** The verified path decodes
  to GPU textures in `cine-shell` and presents without leaving the GPU. A
  browser would need a second presentation path, and there is none. What the
  browser *can* do honestly is request individual decoded frames as images —
  frame-accurate, not real-time — and it must say which of those it is doing.
* **Audio, A/V sync: UNAVAILABLE.** No audio device exists on this build.
* **Scopes, colour management, HDR: UNAVAILABLE.** See `PHASE-3-STATUS.md`.
* **Editing operations from the browser: PARTIAL.** Ripple/roll/slip/slide etc.
  exist and are tested in `cine-timeline`, but no RPC reaches them yet, so the
  interface must not offer them.

## The demo project is empty of exactly what needs showing

Measured against the running runtime (`stats`, revision 24, 94 nodes):

```
audio.* 15   comp.* 24   logistics.* 14   previs.* 15   script.* 22   project 1
nle.*  0     asset.media 0
```

There is **no timeline and no media in the project**. A timeline pane fed from
this project would correctly show nothing. So part of this work is making the
fixture real: ingesting the verified media file and importing a cut, through the
same transactional path any other client would use — not by fabricating nodes.

## Consequences for the plan

1. Extend the protocol (E) rather than inventing a parallel model.
2. Keep one source of truth: the browser holds a *projection* of the graph and
   sends intents back; it never computes composition maths locally.
3. Represent (F) honestly in the interface using the existing capability model.
4. Make the end-to-end fixture a visible workflow, as the directive asks, by
   putting real media and a real cut into the project through real transactions.
