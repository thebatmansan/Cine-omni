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
cargo test --workspace
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
* A cross-check is not an oracle. When two implementations disagree, adjudicate
  against something exact before deciding which is wrong — the true-peak
  "defect" was real divergence with the blame reversed.
