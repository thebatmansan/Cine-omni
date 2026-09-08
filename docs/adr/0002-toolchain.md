# ADR 0002 — Rust core, wgpu GPU layer, TypeScript shell

**Status**: accepted

## Decision

* **Rust** for every engine and domain crate. Reasons: deterministic
  performance for image/audio kernels, fearless parallelism (rayon) for
  reactors and node evaluation, one code base for native desktop and WASM.
* **wgpu + WGSL** for the GPU: one API on Vulkan/Metal/DX12/WebGPU. Kernels are
  embedded and validated with `naga` in tests, so CI without a GPU still
  catches shader errors. The `wgpu` feature is off by default to keep
  workspace builds fast; every kernel has a CPU mirror used as the reference.
* **TypeScript + React** for the shell in `ui/`, talking JSON-RPC to the
  `cine serve` daemon (or WASM bindings in the browser build). Panels are thin:
  all state is in the graph, all math is in Rust.
* **Interchange is native**: Fountain, FDX, OTIO JSON, USDA, OCIO config,
  `.cube`, CDL XML, WAV are implemented in-tree rather than through C
  bindings, so the workspace builds with no system libraries.

## Toolchain note

The development machine had no system C compiler. Rust was installed via
rustup and **Zig** (`zig cc`) is used as the linker/C compiler through
`~/.cargo/config.toml`:

```toml
[target.x86_64-unknown-linux-gnu]
linker = "/home/<user>/.local/bin/zigcc"   # exec zig cc -target x86_64-linux-gnu.2.39 "$@"
[env]
CC = "/home/<user>/.local/bin/zigcc"
```

Any machine with a normal `cc` needs none of this.
