# CINE-OMNI — working notes for agents

Rust workspace (`crates/*`) + TypeScript UI (`ui/`). Read `docs/ARCHITECTURE.md`
and `docs/MODULE-CONVENTIONS.md` before changing module code.

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
  RwLock is not reentrant and the test will deadlock silently.
* Graph writers must be idempotent: re-running with the same input produces
  zero ops (`group.deltas.is_empty()`). Reactor convergence depends on it.
* Node kinds / attribute keys come from `ucg_schema::{kinds, attrs}`.
* Derived entity ids: `EntityId::derived(source, kinds::X)`; importer ids:
  `EntityId::from_key("<importer>:<stable key>")`.
* Reactors live only in `cine-pipeline`; domain crates expose pure functions.
