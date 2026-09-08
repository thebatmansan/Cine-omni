# Module conventions

Every domain crate (`cine-*`) follows the same shape so the pipeline crate and
the UI can treat them uniformly.

## Layout

```
crates/cine-<module>/
  Cargo.toml          depends on ucg-core, ucg-schema (+ workspace deps only)
  src/lib.rs          crate docs, `pub mod`s, `pub use` of the main types
  src/<domain>.rs     pure domain types + algorithms, NO graph dependency
  src/graph.rs        import/export/sync between domain types and the UCG
  tests/*.rs          integration tests (parsers against fixtures, graph round trips)
```

## Rules

1. **Domain logic is pure.** Parsers, optics, DSP, color math take plain
   values and return plain values. They are unit-tested without an `Engine`.
2. **Graph code goes through `Tx`.** Writers take `&mut Tx`, readers take
   `&Graph`. Never hold an `Engine` inside a domain type.
3. **Kinds and attrs come from `ucg_schema`.** Add a constant there if a new
   cross-module name is needed; module-private attrs may be string literals
   prefixed with the module name (`"nle.locked"`).
4. **Derived entities use derived ids.** `EntityId::derived(source, kinds::X)`
   or `EntityId::from_key("<importer>:<stable key>")`. Re-importing or
   re-deriving must be idempotent (same ids, `tx.set` no-ops when unchanged).
5. **Cross-module links are edges**, not attributes:
   * `SyncedTo`   clip → scene/shot, alignment → media
   * `DerivedFrom` previs scene → script scene, shot list → previs scene
   * `DependsOn`  comp graph → shot, grade → clip
   * `Binds`      clip → media asset, character → model
   * `Contains`   hierarchy
6. **Reactors are pure** (`&ChangeSet, &Graph -> Vec<Plan>`) and live in
   `cine-pipeline`, not in the domain crate. Domain crates expose the functions
   reactors call.
7. **Errors**: each crate has its own `Error` (thiserror) with `From<UcgError>`.
8. **Tests**: every public algorithm has a unit test; every graph writer has a
   round-trip test; every parser has a fixture under `tests/fixtures/`.
9. **No `unwrap()` in library code** on user data paths; `expect` only for
   invariants with a message.
10. **Docs**: every `pub` item has a doc comment; `lib.rs` has a crate-level
    overview with an example.

## Building

```
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
cargo test -p cine-<module>
cargo clippy -p cine-<module> -- -D warnings
```
