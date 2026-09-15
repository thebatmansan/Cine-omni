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
| **dependency debuginfo off** | **6.59 GB** | **4.48 GB** | 2.08 GB | 63 |

**30% smaller overall, 36% smaller in `deps`** — and the smaller build contains
*more* test binaries than the larger one, because three test files were added
between the two runs. The reduction is therefore a lower bound, not a best case.

Build *times* from those runs are not comparable and are deliberately not quoted:
the two ran under different amounts of competing load.

The reason it is so large is in the forensics: 159 linked executables measured
11.99 GiB on disk, **95% of it `.debug*` sections**, because Rust test binaries
link statically and every one of them carried its own copy of the dependency
debug information. `rlib`s were 41% debuginfo by the same measurement.

Applied in `Cargo.toml`. The cost is that a debugger cannot step into `wgpu` or
`naga`; workspace crates keep full debug information, and the override to get it
back for one run is in the comment there.

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
| Superseded duplicate artifacts (5.72 GB measured) | **Not addressed** — reclaimable only by a clean, which needs authorisation |
| Test binaries, ~76 MB each and one per test file | **Reduced, not solved** — fewer, larger test files would cost less; not done |
| Op log never compacted | **By design** — history is source of truth; compaction stays an explicit action |
| Orphaned scratch binaries (311 MB) | **Not addressed** — dead output, nothing regenerates it |

---

## 5. What this document did **not** do

Nothing was deleted, pruned, moved or reclaimed. The measured tree is intact.

**One consequence must be stated plainly:** changing the dependency profile
invalidates the existing dependency artifacts, so the next build writes a fresh
set *alongside* the old ones. `target/` will therefore **grow** before it
benefits. Reclaiming the difference needs one `cargo clean`, which is a decision
for whoever owns the machine, not something this work will do on its own.

Recommendations carried forward, none executed:

1. `cargo clean` once, to drop the ~19 GB of accumulated stale artifacts the
   current 28 GB tree is carrying relative to a 6.6 GB cold build.
2. Delete the orphaned `zz_*` scratch binaries in `target/debug/deps` — dead
   output whose sources no longer exist in git or the worktree.
3. Add `du -sh target` to the build documentation. The forensic finding was that
   nobody looked until it was 60 GB.
