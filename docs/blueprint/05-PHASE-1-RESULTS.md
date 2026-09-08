# 05 — Phase 1 Results

**Status: complete.** Every number below was measured on this machine with the
committed benchmarks, before and after. Nothing here is projected.

```bash
cargo test -p ucg-core      --release --test scale -- --ignored --nocapture
cargo test -p cine-pipeline --release --test scale -- --ignored --nocapture
```

---

## A. Graph indexing

`Graph::of_kind()` was **74× slower than having no index at all**, because
`sort_by_key` re-evaluates its key on every comparison and each evaluation
hashed a UUID.

**Fix, in two parts.** The `by_kind` index became an `IndexSet`, so it already
yields insertion order and no sort is needed. `children()` moved to
`sort_by_cached_key`, which evaluates its key once per element.

Then the benchmark exposed something the original analysis had missed: even
without the sort, resolving 8,000 ids costs 8,000 hash lookups, while a
contiguous scan of the node arena costs almost nothing per node. **An index is
not unconditionally better.** So `of_kind` now picks a strategy by selectivity,
the way a query planner does.

Measured at 56,001 nodes:

| Selectivity | Matches | Indexed | Naive scan | Index speedup |
|---|---:|---:|---:|---:|
| 0.0% | 1 | 607 ns | 954 µs | **1571×** |
| 3.6% | 2,000 | 568 µs | 1.44 ms | **2.5×** |
| 14.3% | 8,000 | 1.98 ms | 2.23 ms | 1.1× |
| 21.4% | 12,000 | 2.52 ms | 2.29 ms | 0.9× |

Before the strategy check, the 14.3% case ran at 0.5×, i.e. the index actively
hurt. It now reaches parity there and wins everywhere else.

Against the old implementation on identical data:

| Query | Old | New | Improvement |
|---|---:|---:|---:|
| `of_kind("logistics.shot")` (8,000 matches) | 17.76 ms | 1.22 ms | **14.5×** |
| `of_kind("script.scene")` (2,000 matches) | 5.04 ms | 0.48 ms | **10.6×** |

**Semantics preserved**, locked down by four new regression tests: insertion
order, order-attribute-then-insertion ordering for `children`, order survival
across removal and undo, and exact equality with a naive scan.

One pre-existing behaviour was discovered and documented rather than changed:
undo of a delete re-appends the node at the end rather than restoring its
original index. The old sort keyed on the same `nodes` index, so it behaved
identically; the test now pins it so any future change is deliberate.

---

## B. Incremental logistics

The logistics reactor rescheduled the entire film and regenerated every call
sheet on any scene change, so an O(1) user action cost O(N).

**Three changes, in increasing order of value:**

1. **A fingerprint fast path.** A deterministic FNV-1a hash over exactly what
   the scheduler consumes — per scene: order, number, page eighths, duration,
   int/ext, location, time of day, cast; plus the global constraints. If it
   matches the one stored with the last schedule, the plan returns with **zero
   ops**. FNV rather than `DefaultHasher` because the value is persisted and
   must be stable across processes.
2. **Diff-driven writes.** `write_schedule_incremental` compares against the
   previous schedule and skips days whose content is unchanged, so their strips
   are never re-serialised.
3. **Dependency-aware call sheets.** Only days that actually moved have their
   call sheet regenerated.

Measured, one edit with all seven reactors installed:

| scenes | nodes | before (any edit) | after: schedule-affecting | after: schedule-neutral |
|---:|---:|---:|---:|---:|
| 7 | 195 | 2.15 ms | 2.29 ms | **0.31 ms** |
| 50 | 1,425 | 5.61 ms | 4.62 ms | **0.20 ms** |
| 200 | 5,661 | 19.9 ms | 9.43 ms | **0.24 ms** |
| 500 | 14,137 | 78.5 ms | 31.6 ms | **0.26 ms** |
| 1,000 | 28,261 | 144.3 ms | 67.5 ms | **0.34 ms** |

**Was the scaling problem fixed? Partly, and the distinction is the point.**

- **Schedule-neutral edits are now flat.** 0.31 ms at 7 scenes, 0.34 ms at
  1,000 — a 143× increase in scene count for no measurable cost increase, and
  **173× faster** than the 59 ms those edits used to cost at 1,000 scenes. Most
  editing traffic is in this class: rewording dialogue that does not change its
  length, notes, props, comp and colour work.
- **Schedule-affecting edits improved about 2×** and remain O(N). This is
  **inherent, not a remaining defect**. Packing a shooting schedule is a global
  optimisation: one scene growing can push scenes across every subsequent day.
  You cannot schedule a film without considering every scene. What Phase 1
  removed is the O(N) *graph* work — the writes and call sheets — that used to
  happen on top of it.

A genuine correctness gap surfaced while testing this: a **global constraint
change did not reschedule at all**, because the reactor was not interested in
document-level changes and the fingerprint did not cover constraints. Both are
fixed, and the test that caught it is `global_change_reschedules_everything`.

Eight tests cover the required cases: schedule-neutral change, duration change,
insertion, deletion, reorder, page-count change, global constraint change, and
call-sheet selectivity.

---

## C. Reactor read architecture — premise corrected

The brief asked to replace expensive snapshot cloning in reactor execution.
**An audit found no such cloning.** `Engine::snapshot()` has exactly one
production call site, `Project::save()`, which legitimately needs an owned graph
to serialise. Reactors receive `&Graph` from a read guard scoped inside the
cascade loop and dropped before any commit. The 363 ms clone cost is real but is
paid on explicit save, not on the interactive path.

Reporting that rather than performing a rewrite that would have changed nothing
measurable.

What the item genuinely asked for — *"explicitly verify that no `engine.read()`
guard can remain alive across `engine.transact()`"* — was delivered differently
and, I think, better. That mistake used to be a **silent deadlock**: no panic,
no timeout, no diagnostic. It is documented in `CLAUDE.md` as the most dangerous
rule in the codebase. Commits now acquire the write lock with a timeout and
return `UcgError::WriteLockTimeout`, whose message names the likely cause.

Two tests cover it: one that makes the mistake and asserts a clean error instead
of a hang, and one proving correctly scoped guards and concurrent readers never
trip the timeout. Transaction isolation is unchanged, no unsafe code was added,
and reactors remain pure planning functions.

---

## D. Bounded operation log

`Engine::log` retained every delta for the process lifetime, as did the undo
stack. An audit found `Engine::log()` has **no production consumer**, so
bounding it is safe.

**What changed:**

- `EngineLimits` caps the in-memory delta window (default 4,096) and the undo
  stack (default 1,000). `Engine::log_dropped()` reports evictions rather than
  hiding them.
- **Durability is untouched.** The on-disk op log remains complete, and undo
  history is rebuilt from it on open, so nothing needed for recovery is dropped.
- **Recovery now distinguishes a torn tail from corruption.** A truncated final
  record is the signature of a process dying mid-append; that record was never a
  completed transaction, so it is discarded and reported via `OplogReport`. A
  malformed line *anywhere else* means later deltas may depend on a missing
  operation, so recovery fails loudly rather than silently reconstructing a
  partial graph. Previously **any** malformed byte made a project unopenable —
  a power loss mid-write could lose the entire project.

**The compaction boundary, stated explicitly:** `save()` *archives* the live
op-log segment rather than deleting it, so undo spans the snapshot.
`compact()` is the single operation that discards pre-snapshot history, and it
never changes current state.

Four tests cover restart, replay, archive, compaction, undo after reload, torn
tail, and mid-file corruption.

---

## E. cargo-deny

Policy in `deny.toml`, wired into CI. All four checks pass:

```
advisories ok, bans ok, licenses ok, sources ok
```

**`licenses ok` is the one that matters architecturally.** It confirms no
GPL-family code is in the dependency graph, which is the boundary the whole
integration strategy rests on.

Three findings, each investigated rather than silenced:

| Finding | Investigation | Resolution |
|---|---|---|
| `RUSTSEC-2024-0436`: `paste` unmaintained | Not a vulnerability. Transitive via `paste → metal → wgpu-hal → wgpu`. `metal` is the Apple backend, not compiled on our targets. Advisory states "No safe upgrade is available". | Documented ignore with the dependency path, the reason it is not actionable, and the condition to revisit: a wgpu release without `paste`. |
| Wildcard dependencies on all 12 crates | `ucg-core.workspace = true` resolves to a path dependency with no version, which registers as a wildcard. Path deps are built from this tree, never fetched. | `allow-wildcard-paths = true`. Registry wildcards are still denied. |
| The above still failed | cargo-deny only exempts path deps for *unpublishable* crates, and no crate declared `publish`. | Added `publish = false`, which is simply accurate: these are workspace-internal crates. Corrected metadata, not weakened policy. |

The policy denies GPL, LGPL and AGPL by omission from the allow list, denies
unknown registries and all git dependencies, and records why `gtk` is banned.

---

## Validation

| Check | Result |
|---|---|
| `cargo test --workspace` | **442 passed, 0 failed** (was 424) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo clippy --workspace --all-targets --all-features` (incl. wgpu) | clean |
| `cargo fmt --all -- --check` | clean |
| `cargo deny check` | advisories ok, bans ok, licenses ok, sources ok |
| `tsc --noEmit` | clean |
| `vitest` | 2 passed |
| `npm run build` | succeeds |

---

## Remaining architectural risks

1. **Schedule-affecting edits are still O(N)** and reach 67 ms at 1,000 scenes.
   Inherent to global schedule packing, but the constant could come down by
   caching per-scene strips instead of rebuilding all breakdowns. Deferred
   because the win is bounded and the complexity is not.
2. **Bulk import is still O(N) and slow** — 2.1 s for 1,000 scenes. Untouched
   by Phase 1; it is the same aggregate-recomputation shape seen through the
   import path.
3. **The single global write lock** remains. Correct today; it will not survive
   twenty workspaces plus background workers plus collaboration.
4. **`snapshot()` is still an O(n) clone** on save. Not on the interactive path,
   so deliberately left alone.
5. **The bulk-data boundary is designed but not implemented.** Per-frame and
   time-series data still have nowhere to go but the graph.
6. **Benchmarks are `#[ignore]`d**, so CI will not catch a regression that
   reintroduces the 74× defect. A performance gate is the obvious follow-up now
   that the targets exist.
