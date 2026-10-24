# Setting up a fresh machine

Everything needed to rebuild CINE-OMNI from a clone of this repository. What the
repository does *not* contain is listed at the end, with how to get it back.

Recorded from the development laptop on 2026-10-24: Ubuntu noble, x86_64, no
system C compiler, no sudo.

## 1. Toolchain

| Tool | Version used | Install |
|---|---|---|
| Rust | 1.98.1 stable (pinned by `rust-toolchain.toml`) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| Zig | 0.16.0 | unpack the release tarball to `~/.local/zig` (only needed without a system `cc`) |
| Node | 24.22.1, npm 11.19.1 | any Node 24 LTS |

With a system compiler (`sudo apt install build-essential`) skip the Zig step.
Without one, Zig is the linker and C/C++ compiler (`docs/adr/0002-toolchain.md`):

`~/.local/bin/zigcc`
```sh
#!/bin/sh
exec "$HOME/.local/zig/zig" cc -target x86_64-linux-gnu.2.39 "$@"
```

`~/.local/bin/zigcxx`
```sh
#!/bin/sh
exec "$HOME/.local/zig/zig" c++ -target x86_64-linux-gnu.2.39 "$@"
```

`chmod +x ~/.local/bin/zigcc ~/.local/bin/zigcxx`, then `~/.cargo/config.toml`
(replace `/home/USER` with your home directory; cargo does not expand `~`):

```toml
[target.x86_64-unknown-linux-gnu]
linker = "/home/USER/.local/bin/zigcc"

[env]
CC = "/home/USER/.local/bin/zigcc"
CXX = "/home/USER/.local/bin/zigcxx"
```

## 2. Build

```
git clone https://github.com/thebatmansan/Cine-omni.git && cd Cine-omni
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
CARGO_INCREMENTAL=0 cargo build -p cine-cli -j 1   # -j 1: a cold build OOMs at default parallelism on 15 GB
cd ui && npm ci && npm run build && cd ..
```

Then the build and test commands in `CLAUDE.md`.

## 3. The demo project

`fixtures/demo-project/` is *The Harbour* (revision 40) as it stood in
`~/cine-projects/demo`: graph snapshot, op log, job journal and derived
waveforms. Its `cache/` and `.cine/runtime.lock` were left out — the cache is
regenerable and the lock belongs to a running process. Copy it out before
opening it, so the running project does not write into the repository:

```
mkdir -p ~/cine-projects && cp -r fixtures/demo-project ~/cine-projects/demo
target/debug/cine start ~/cine-projects/demo --port 7331
```

Its clips refer to source media by absolute path (see below); without the files
the project opens and the media reports as offline.

## 4. Not in the repository

| What | Expected at | Needed by |
|---|---|---|
| `Super_cinematic_image_to_vid.mp4`, `A_solitary_monochrome_crimson.mp4` | `~/Downloads/` | the real-media tests in `cine-media`, `cine-runtime`, `cine-pipeline`, `cine-shell`, `cine-gpu`; they skip loudly when absent |
| `make_him_looking_at_the_Black.mp4`, `mp4.mp4` | `~/Downloads/` | the demo project's clips |
| `forest.exr` | `~/tools/blender-4.2.4-linux-x64/4.2/datafiles/studiolights/world/` | regenerating `crates/cine-comp/tests/data/exr-oracle.tsv` only (ships with Blender 4.2.4) |

The media paths are hard-coded to `/home/finstein-emp/`; on a machine with a
different user name, put the files there or edit the paths in the tests.
