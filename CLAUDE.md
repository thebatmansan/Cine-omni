# CINE-OMNI — working notes for agents

Rust workspace (`crates/*`) + TypeScript UI (`ui/`). Read `docs/ARCHITECTURE.md`
and `docs/MODULE-CONVENTIONS.md` before changing module code.

## Run the application
```
cine start                 # from a project directory — this is the ONE command
cine start /path/to/proj   # a project elsewhere
```
`cine start` opens the project, serves the UI and the WebSocket API from one
process, and owns the project lock. A second `cine start` in the same directory
attaches to the running one rather than starting a rival — two runtimes would
interleave two op logs and corrupt the history. See
`docs/blueprint/APPLICATION-RUNTIME.md`.

## Build & test
```
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"   # this machine: rustup + zig linker
CARGO_INCREMENTAL=0 cargo test --workspace -j 1   # -j 1: see the storage rule
# This machine runs out of memory linking the workspace's test binaries at
# default parallelism and at -j 2. Testing a few crates at a time also works.
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p cine-cli -- demo
cd ui && npm test && npm run build
```

## Rules that bite
* Never hold an `engine.read()` guard across `engine.transact(...)`; the
  RwLock is not reentrant. This no longer hangs silently: the commit times out
  and returns `UcgError::WriteLockTimeout` naming the likely cause. Still scope
  every read guard in its own block.
* Graph writers must be idempotent: re-running with the same input produces
  zero ops (`group.deltas.is_empty()`). Reactor convergence depends on it.
* Node kinds / attribute keys come from `ucg_schema::{kinds, attrs}`.
* Derived entity ids: `EntityId::derived(source, kinds::X)`; importer ids:
  `EntityId::from_key("<importer>:<stable key>")`.
* Reactors live only in `cine-pipeline`; domain crates expose pure functions.
* `cine-media`: never make a capability claim the build cannot back. Codec
  support is decided in one place (`probe::is_decodable` + the profile gate);
  a codec that is not handled returns `Error::NoDecoder` naming the real
  alternatives, never a blank frame. Time is exact rational — floats are for
  display only, never for storage, comparison or seek. Read
  `docs/blueprint/PHASE-2-MEDIA-ENGINE.md` before touching it.
* Media tests use real files and `need!` to skip loudly when absent. A media
  test that quietly passes with no media is worse than no test.
* JSON-RPC lives in `cine-runtime::rpc` and nowhere else. Both transports
  (`cine start` over WebSocket, `cine serve` over stdio) dispatch through it;
  adding a method to one transport only recreates the two-truths defect.
* Time is `ucg_core::RationalTime`: integer ticks at an exact rational `Rate`.
  There is exactly one time model — `cine-media` re-exports it rather than
  defining a second. Note the historical trap: the old media `TimeBase` was a
  *period*, so `TimeBase::new(1, 48000)` meant 48 kHz; `Rate` is a *rate*, so
  the equivalent is `Rate::whole(48000)`. Never construct a rate from a decimal
  (`29.97` is not `30000/1001`), never derive a scale factor by dividing two
  rates, and never reach for `f64` seconds except to display. `rescale_checked`
  reports whether a conversion was exact — ignoring that flag is a choice to
  lose information.
* Playback is `cine_media::Scheduler`: the clock decides which picture is due,
  the scheduler decides whether to present, hold, drop or refuse, and the event
  loop sleeps on `next_deadline()` rather than on a frame interval. Never
  advance a playhead by counting frames, and never report a transport as
  `Playing` while frames are being dropped — `Degraded` exists for that. Read
  `docs/blueprint/PLAYBACK.md`; it records what is measured and what is not
  claimed.
* Decoding happens on a worker thread (`cine_media::DecodeAhead`), never on the
  presentation thread. Two things follow. Decode-ahead does not make a decoder
  faster — without a pre-roll it measures no better than synchronous decoding,
  so a transport that starts playing must pre-roll a cushion first. And the
  buffer is bounded by *bytes*, checked before decoding rather than after:
  checking after let a picture that did not fit be decoded, evicted, and decoded
  again forever.
* Storage: `CARGO_INCREMENTAL=0` for whole-workspace builds and tests, and
  **never two `cargo` processes against one `target/`** — that combination
  corrupted the incremental cache into 20 GB of unusable sessions and
  `undefined hidden symbol` link errors. Per-crate edit loops may keep
  incremental on. Dependencies build without debuginfo (30% smaller, measured);
  workspace crates keep it. `du -sh target` occasionally — the last time nobody
  looked it reached 60 GB. Read `docs/blueprint/STORAGE-POLICY.md` before adding
  anything that writes to disk: every byte belongs to a class, and only caches
  and temporary files may ever be deleted automatically.
* A backend capability is not finished until there is a path to it: protocol →
  frontend state → visible interface → interaction → test. The frontend holds a
  *projection* of the graph and sends intents back; it never computes
  composition, never writes project state locally, and never claims a capability
  the runtime did not report. Read `docs/blueprint/APPLICATION.md`. Where
  something is backend-only by necessity, write down why — real-time playback is
  native because a frame must not leave the GPU, and `capabilities` says so.
* A cross-check is not an oracle. When two implementations disagree, adjudicate
  against something exact before deciding which is wrong — the true-peak
  "defect" was real divergence with the blame reversed.
