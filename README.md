# CINE-OMNI

One unified cinema production system: screenwriting, storyboarding, 3D previs,
production logistics, on-set metadata, non-linear editing, VFX compositing,
color grading and audio post, all built on a single reactive **Unified Context
Graph (UCG)**.

Every scene, line of dialogue, camera, clip, comp node, grade and audio clip is
one node with one UUID. Dependency edges form a DAG; a change anywhere is
propagated to every dependent module by pure, parallel *reactors*, and the whole
cascade is one undo step.

```
$ cine demo
== imported sample.fountain ==
revision 31 — 251 nodes, 358 edges, 7 reactors
  audio.clip                      27
  comp.graph                       7
  logistics.call_sheet             2
  logistics.shot                   7
  previs.camera                    7
  script.dialogue                 27
  ...
== editing first dialogue line of scene 1 ==
  was: "We're closed."
transaction `edit dialogue` → 7 deltas
    32  depth 0  edit dialogue                                      1 ops
    33  depth 1  script: recompute scene stats                      3 ops
    34  depth 1  audio: sync dialogue placeholders                  6 ops
    35  depth 2  previs: sync previs scene                          2 ops
    36  depth 2  logistics: rebuild breakdown + shot list           3 ops
    37  depth 2  logistics: reschedule                              3 ops
    38  depth 3  comp: ensure comp for shot                         1 ops
SCENE 1 INT. HARBOR OFFICE - NIGHT
  words=58 eighths=7 est=31.3s characters=["MAREN", "ELIAS"]
  previs: INT. HARBOR OFFICE - NIGHT cameras=1 characters=2
  shot list: 1 shots
    shot 1A Full Static 35mm est=31.3s comp=yes stale=true
  audio: dialogue placeholders=5

== undo ==
undid `edit dialogue` in one step (19 inverse ops)
```

## Layout

| Crate | What it is |
|---|---|
| `crates/ucg-core` | The graph engine: nodes, typed values, DAG edges, transactions, undo groups, reactive cascade, JSON/MessagePack snapshots + op log |
| `crates/ucg-schema` | Shared vocabulary: node kinds, attribute keys, typed components (Transform, CameraParams, LightParams, ClipRef, ColorMeta, ShotSpec, …) |
| `crates/cine-script` | Fountain + FDX bidirectional parser/writer, auto-tagging, page/duration stats, ScriptSync word alignment |
| `crates/cine-previs` | Physically accurate optics (sensors, FOV, DoF), physical lights, USD-style stage with USDA export, crane/dolly rigs, grease-pencil strokes on frustum planes, 2D storyboard canvas with onion skin and light table |
| `crates/cine-logistics` | Breakdown sheets, shot lists + lens grids from previs cameras, stripboard scheduler, call sheets, 240 Hz on-set metadata ingestion |
| `crates/cine-timeline` | OpenTimelineIO model + lossless JSON I/O, ripple/roll/slip/slide/insert/overwrite/lift/extract, FFT waveform sync |
| `crates/cine-comp` | Node compositor: merge, keyers, roto, blur, transform, deep compositing, Z-defocus from previs depth, procedural generators, cached parallel evaluator |
| `crates/cine-color` | ACES (AP0/AP1, ACEScc/cct, RRT/ODT), camera log curves, OCIO v2 config parser, .cube LUTs (tetrahedral), CDL, wheels, curves, HSL qualifiers, masks, SDR/HDR scopes |
| `crates/cine-audio` | DAW session model, block renderer, VBAP spatial panning up to 7.1.4, stem delivery, LUFS metering, spectral-gating dialogue isolation, WAV I/O |
| `crates/cine-gpu` | wgpu device + WGSL kernels (merge, CDL, 3D LUT, scopes, blur, keyer, DoF, PBR previs viewport), frame graph with resource aliasing; kernels validated by naga in CI |
| `crates/cine-pipeline` | The reactors that wire every module together, and the `OmniProject` façade |
| `crates/cine-cli` | `cine` binary: project management, import/export, cascade demo, JSON-RPC daemon |
| `ui/` | TypeScript + React shell talking to the daemon (viewport, timeline, node editor) |
| `docs/` | Architecture, module conventions, ADRs |

## Build

Rust 1.85+ (stable). No system C compiler is required if Zig is used as the
linker (see `docs/adr/0002-toolchain.md`). Setting up a fresh machine, and what
the repository does not contain: `docs/SETUP.md`.

```
cargo test --workspace            # 424 tests, all green
cargo run -p cine-cli -- demo     # the thesis walkthrough
cargo build -p cine-cli --features wgpu   # with the real GPU backend
cargo test -p cine-gpu --features wgpu    # validates kernels against the CPU reference
cine gpu                                  # list kernels and probe the adapter
```

## Using a project

```
cine new ~/Films/Dunes.cineomni --name "Dunes"
cine import ~/Films/Dunes.cineomni dunes.fountain
cine import ~/Films/Dunes.cineomni cut_v1.otio
cine report ~/Films/Dunes.cineomni
cine edit ~/Films/Dunes.cineomni 0 0 "We ride at dawn."      # scene 0, line 0
cine export ~/Films/Dunes.cineomni usda --scene 0 --out scene1.usda
cine export ~/Films/Dunes.cineomni callsheets
cine undo ~/Films/Dunes.cineomni       # works across processes (history is in the op log)
cine serve ~/Films/Dunes.cineomni      # JSON-RPC for the UI
```

A project is a directory: `project.json`, `graph.json` (+ `graph.msgpack`),
`oplog/*.jsonl`, `assets/`. Every commit is appended to the op log and `save`
writes a fresh snapshot, archiving the log segment rather than dropping it, so
undo and redo still work after quitting and reopening.

See `docs/ARCHITECTURE.md` for the design.
