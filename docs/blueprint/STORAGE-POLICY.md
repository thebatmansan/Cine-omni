# Storage policy and budget

**Status: PARTIAL.** The classification and the build policy are decided and, for
the build, applied and measured. The project-side lifecycle for derived media is
**designed, not implemented** — there are no proxies, renders or caches on disk
yet — and this document says which is which.

`STORAGE-FORENSICS.md` measured where the space went. This says where it is
allowed to go, and what must never be deleted to get there.

---

## 1. The classes

Every byte the system writes belongs to exactly one class, and the class decides
what may happen to it.

| Class | May be deleted automatically? | Example |
|---|---|---|
| **Source of truth** | **Never** | `graph.msgpack`, `project.json`, the op log |
| **User media** | **Never** | the camera original the graph references |
| **Project derived — retained** | Never without an explicit instruction | a delivered master |
| **Project derived — regenerable** | Only on explicit instruction | proxies, optimised media |
| **Cache** | Yes, under a bound | decoded frames, thumbnails, waveforms |
| **Reproducible artifact** | Yes, on instruction | `target/`, `ui/dist` |
| **Temporary** | Yes, when not in use | `.cine/runtime.lock` |

Two rules follow from the table and are worth stating on their own.

**A derivative is never promoted to a source.** `DerivativeKind` already carries
this: a proxy references its source's content hash and the recipe that made it.
Losing the original and keeping the proxy is data loss, and nothing in the system
may treat the proxy as a replacement.

**A render is not a cache.** It is derived, and it is also a deliverable that
someone may have signed off. `DerivativeKind::Render` is therefore *retained*
derived media: regenerable in principle, never disposable in practice.

### Where history sits

Archived op-log segments are **source of truth**, not cache. They are the undo
history. `Project::compact()` deletes them and still has no callers, which
`STORAGE-FORENSICS.md` listed as an omission. It is an omission in the *interface*
— compaction must be an explicit, confirmed user action — not something to wire
into a save path. A save that silently discarded history would be the single
most destructive automatic behaviour in the system.

---

## 2. The build, measured

Controlled comparison: a cold `cargo test --workspace --no-run` into a fresh,
empty target directory, one configuration at a time.

| Configuration | Total | `deps` | incremental | test exes |
|---|---:|---:|---:|---:|
| baseline (dependency debuginfo on) | 9.41 GB | 7.04 GB | 2.08 GB | 61 |
| **dependency debuginfo off** (applied) | **6.59 GB** | **4.48 GB** | 2.08 GB | 63 |
| + workspace `line-tables-only` (opt-in) | **3.74 GB** | 2.37 GB | 1.35 GB | 64 |

**30% smaller overall and 36% in `deps`** for the applied setting — and a lower
bound, not a best case, because the smaller builds contain *more* test binaries
than the baseline did (test files were added between runs).

Going further, to `line-tables-only` on the workspace crates too, is **60%
smaller** and also cuts the incremental directory by a third. It is *not* the
default, on purpose: it keeps file-and-line in panic backtraces but removes
variable inspection in a debugger **for our own code**, and that is a working
tool, not a build artifact. It is offered as a choice rather than imposed:

```
cargo test --config 'profile.dev.debug="line-tables-only"'
```

Whoever owns the machine can make that the default; this work will not make it
for them.

Build *times* from those runs are not comparable and are deliberately not quoted:
the two ran under different amounts of competing load.

The reason it is so large is in the forensics: 159 linked executables measured
11.99 GiB on disk, **95% of it `.debug*` sections**, because Rust test binaries
link statically and every one of them carried its own copy of the dependency
debug information. `rlib`s were 41% debuginfo by the same measurement.

Applied in `Cargo.toml`. The cost is that a debugger cannot step into `wgpu` or
`naga`; workspace crates keep full debug information, and the override to get it
back for one run is in the comment there.

### Memory, which turns out to be the binding constraint

Measured on this machine, twice: `cargo test --workspace` was killed for running
the system out of memory at default parallelism **and again at `-j 2`**. It is a
two-core laptop with 15 GiB, 4 GiB of swap, and several gigabytes already held
by other software; linking Rust test binaries that are hundreds of megabytes of
debug information is what spikes it.

That makes the debuginfo settings above a *memory* policy as much as a disk one:
smaller objects link in less memory. Until that is measured, the working rule on
this machine is

```
cargo test --workspace -j 1          # or test a few crates at a time
```

and the honest caveat that whole-workspace numbers in this session were gathered
per-crate for that reason.

### Incremental compilation

Unchanged by the above: 2.08 GB in both configurations on a *cold* build. The
forensics measured the real problem, which is not the cold size but the
accumulation — 235 session directories for 58 compilation units, because rustc
garbage-collects within a session directory and never across them. That is what
produced the ~20 GB that had to be deleted.

The policy is therefore about *when* it is on, not how big it is:

* **Keep it** for edit-compile loops on one crate, which is what it is for.
* **`CARGO_INCREMENTAL=0` for whole-workspace runs**, which are the ones that
  spray session directories across every crate at once. Every `cargo` invocation
  in this session used it.
* **Never run two `cargo` processes against one `target/`.** That is the
  documented cause of the corruption that started the investigation. A private
  `CARGO_TARGET_DIR` avoids the corruption at the price of a full duplicate
  tree — measured here at 6.6–9.4 GB per copy.

---

## 3. The budget

The target is that a complete development checkout stays reasonably below
**100 GB** without giving up professional capability.

| Consumer | Steady state | Notes |
|---|---:|---|
| Source, docs, tests, fixtures | ~6 MB | measured; 0.03% of the current tree |
| Git objects | ~5 MB | 659 loose objects, no packs |
| `ui/node_modules` | ~76 MB | reproducible from the lockfile |
| `target/` (dev + release) | **7–9 GB** | measured cold, with the new profile |
| Decoded-frame cache | ≤ 512 MiB | bounded by bytes, in memory, `AheadConfig` |
| **Development total** | **~8–10 GB** | |

That leaves roughly 90 GB of headroom, and the honest statement about it is that
**the headroom is for media, and media is the thing not yet on disk.** A
professional project's weight is proxies, render caches and conformed copies. The
architecture keeps originals *out* of the project — the graph references them by
content hash — which is what makes the number achievable; it does not make it
automatic.

### What must exist before derived media lands

Each of these is a gate, not a wish. None is implemented.

1. **A location.** Derived media belongs under a project-local `derived/<kind>/`
   and caches under `cache/`, so a survey can classify by path rather than guess.
2. **A bound on every cache.** A cache without a size cap and an eviction policy
   is not a cache. The decoded-frame cache and the decode-ahead buffer both have
   one; a disk-backed frame or render cache does not exist and must not ship
   without one.
3. **Stale detection.** `Derivative` already carries `source_hash` and `recipe`;
   a derivative whose source or recipe has changed is stale and must be visibly
   stale rather than silently wrong.
4. **An explicit release path.** Deleting regenerable derived media is a user
   action with a report of what will go, never a background sweep.

---

## 4. Growth mechanisms, and which are now addressed

From `STORAGE-FORENSICS.md`, with the current status of each:

| Mechanism | Status |
|---|---|
| Dependency debuginfo in every static test binary | **Addressed** — 30% measured |
| Incremental sessions accumulating per configuration | **Policy only** — `CARGO_INCREMENTAL=0` for workspace runs; nothing enforces it |
| Superseded duplicate artifacts (5.72 GB measured) | **Reclaimed once, and it came back** — see §6 |
| Test binaries, ~76 MB each and one per test file | **Reduced, not solved** — fewer, larger test files would cost less; not done |
| Op log never compacted | **By design** — history is source of truth; compaction stays an explicit action |
| Orphaned scratch binaries (311 MB) | **Gone with the clean in §6**, and nothing prevents them returning |

---

## 5. What this document did **not** do

Nothing was deleted, pruned, moved or reclaimed. The measured tree is intact.

**One consequence must be stated plainly:** changing the dependency profile
invalidates the existing dependency artifacts, so the next build writes a fresh
set *alongside* the old ones. `target/` will therefore **grow** before it
benefits. Reclaiming the difference needs one `cargo clean`, which is a decision
for whoever owns the machine, not something this work will do on its own.

Recommendations carried forward (see §6 for the one since executed):

1. `cargo clean` once, to drop the ~19 GB of accumulated stale artifacts the
   current 28 GB tree is carrying relative to a 6.6 GB cold build.
2. Delete the orphaned `zz_*` scratch binaries in `target/debug/deps` — dead
   output whose sources no longer exist in git or the worktree.
3. Add `du -sh target` to the build documentation. The forensic finding was that
   nobody looked until it was 60 GB.

---

## 6. The one clean, and what it measured

`target/` was surveyed again and had reached **92 GB** — `target/debug` 90 GB
of it, with 21 GB in `incremental` alone — against the 7–9 GB this document
targets. The disk was 62% full with 85 GB free: not yet a failure, but eleven
times the intended size, and the second time the tree has been found far past
its bound because nobody looked.

With the machine owner's authorisation, one `cargo clean` was run:

```
Removed 96709 files, 96.4GiB total
disk 62% used (85 GB free)  ->  21% used (176 GB free)
```

Three things this does and does not establish:

* **It is reproducible, not a loss.** Everything removed is rebuilt by
  `cargo build`; nothing under `.cine/` or the worktree was touched. That is
  why `target/` is a *reproducible artifact* in the table in §1 and why the
  only question was whose decision it was, not whether it was safe.
* **Nothing prevents a third time.** The growth mechanisms in §4 are unchanged;
  `CARGO_INCREMENTAL=0` is policy, not enforcement, and every test file still
  links its own binary. The clean bought headroom, it did not fix a cause.
* **`du -sh target` belongs in the loop, not in a document.** The finding that
  matters is the one in §4's last row: the tree was measured twice, at 60 GB
  and at 92 GB, and both times only because somebody happened to look.

## 7. The first derived media on disk: renders

`render.sequence` (`crates/cine-runtime/src/render.rs`) is the first thing that
writes derived media into a project. Against the gates in §3:

1. **Location — met.** `derived/render/<sequence>/<unix seconds>-job<id>/`,
   `frame_%06d.png` plus `manifest.json`. A render is written into a sibling
   `….partial` directory and renamed only when every picture and the manifest
   exist, so a directory without the suffix is complete. A cancelled or failed
   render's partial directory is *temporary* data and is removed by the render
   itself; nothing else ever deletes under `derived/`.
2. **Bound — not applicable, deliberately.** A render is retained derived media,
   not a cache (§1). Its size is known before it starts — frames × width ×
   height, compressed — and it is not evicted.
3. **Stale detection — met.** The manifest records one fingerprint per
   dependency the render read (the clips in its range, each file's content
   hash, the captions, the colour) and the renderer version; the `renders`
   call recomputes them and re-hashes the files, and the Jobs tab shows each
   render `current`, `stale` (with what changed), `invalid` (files missing) or
   `failed`. An unrelated change does not make a render stale
   (`render_movie.rs`).
4. **Release — manual only.** There is no delete path in the application. A
   render is removed by a person, on disk.

A movie render is one `movie.mov` beside its manifest, written streaming:
only the sample tables are held in memory. Measured sizes, 320×180 at
quality 18: 48 frames with 48 kHz 24-bit stereo sound, 582 KB — of which
576 KB is the sound (96,000 sample frames at 6 bytes each) and 6 KB the
picture of still bars and the index. Uncompressed sound dominates short
low-resolution renders; it is the right trade for a deliverable whose sound
must not be altered.

## 8. Proxies on disk

`media.proxy` (`crates/cine-runtime/src/proxy.rs`) writes to
`derived/proxy/<original hash, 16 chars>/<recipe key, 12 chars>/proxy.mov`
with a `manifest.json` beside it. Against §3's gates:

1. **Location — met.** One directory per original and recipe, so a survey can
   tell whose proxy a file is without opening it. Written into a sibling
   `….partial` and renamed when complete; a cancelled or failed proxy's partial
   directory is temporary data and is removed, along with the empty
   directories it needed.
2. **Bound — not applicable.** A proxy is retained derived media, not a cache:
   it is made when asked for and removed when a person removes it.
3. **Stale detection — met.** A proxy records the original's content hash, its
   recipe with a version, and its own hash and size; `current`/`stale`/
   `missing` is recomputed whenever the media is listed, and playback never
   reads anything but `current`.
4. **Release — manual only.** Remaking a proxy of the same recipe replaces it
   (the new one is complete before the old one goes). Nothing else deletes a
   proxy; there is no automatic sweep.

Measured: a 640×360 proxy of a 24-picture 1280×720 original is 0.3 MB, of which
the 24-bit stereo sound is 288 KB. Uncompressed sound dominates short proxies,
as it does short renders.
