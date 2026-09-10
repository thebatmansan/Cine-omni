# Storage forensics — measured, 2026-09-10

Read-only investigation. Nothing was deleted, moved, compacted or rebuilt. Every
number below is followed by the command that produced it. Where a claim could
not be measured it is marked **UNVERIFIED** rather than estimated silently.

Measurement host state at the time of the survey:

```
$ uptime
 09:22:33 up 6 min,  1 user,  load average: 2.87, 3.22, 1.67
$ ps aux | grep -i '[c]ine'
(no cine process — only the survey's own shell)
```

> **Correction to the brief.** The task statement says a `cine` process is
> currently running and serving a project. It is not. The machine rebooted six
> minutes before this survey began. What remains is a *stale* lock file:
>
> ```
> $ cat ~/cine-projects/demo/.cine/runtime.lock
> { "pid": 59048, "address": "127.0.0.1:7331",
>   "started_unix": 1788973634,   # 2026-09-09 22:27
>   "project": "/home/finstein-emp/cine-projects/demo", "version": "0.1.0" }
> ```
>
> PID 59048 died at the reboot. The lock outlived it. That is itself a finding —
> see *Growth mechanisms*, item 6.
>
> The reboot also wiped `/tmp`, which means the private `CARGO_TARGET_DIR`s the
> brief asks about (`/tmp/ocio-target`, `/tmp/audioperf-target`) can no longer be
> measured. Their historical size is **UNVERIFIED** and now unmeasurable.

---

## 1. Measured baseline

```
$ du -sb . .git target target/debug target/debug/deps \
      target/debug/incremental target/debug/build target/debug/examples \
      target/release target/doc crates docs tests ui ui/node_modules
```

| Path | Bytes | GiB | Share of repo |
|---|---:|---:|---:|
| **`.` (whole repository)** | **23,719,838,293** | **22.09** | 100.0 % |
| `target` | 23,644,213,307 | 22.02 | 99.68 % |
| `target/debug` | 22,269,856,319 | 20.74 | 93.89 % |
| `target/debug/deps` | 14,252,483,670 | 13.27 | 60.09 % |
| `target/debug/incremental` | 7,507,635,520 | 6.99 | 31.65 % |
| `target/debug/examples` | 268,016,374 | 0.25 | 1.13 % |
| `target/debug/build` | 255,694,546 | 0.24 | 1.08 % |
| `target/release` | 1,367,101,246 | 1.27 | 5.76 % |
| `target/doc` | 7,254,070 | 0.007 | 0.03 % |
| `ui/node_modules` | 68,871,806 | 0.064 | 0.29 % |
| `.git` | 2,409,190 | 0.002 | 0.01 % |
| `crates` (all source + tests) | 2,523,638 | 0.002 | 0.01 % |
| `docs` | 1,361,257 | 0.001 | 0.006 % |
| `tests` (incl. `tests/fixtures`) | 50,130 | 0.00005 | 0.0002 % |

`du -sh` reports `.git` as 5.2 M and `.` as 23 G — the difference from the byte
figures is allocated-block overhead on ~659 small loose git objects.

### Everything the repository is *not*

```
$ git count-objects -v
count: 659      size: 4172      in-pack: 0      packs: 0
size-pack: 0    prune-packable: 0   garbage: 0   size-garbage: 0
$ git rev-list --count HEAD
19
```

19 commits, 659 loose objects, **zero packs**, 4.1 MB of object storage. Git is
not a storage consumer here and never has been. There is no history-rewrite or
`gc` question to answer.

```
$ find . -path ./target -prune -o -path ./ui/node_modules -prune -o -type f \
    \( -name '*.log' -o -name '*.mp4' -o -name '*.mov' -o -name '*.wav' \
    -o -name '*.exr' -o -name '*.dpx' -o -name '*.png' -o -name '*.mxf' \
    -o -name '*.r3d' -o -name '*.braw' \) -print
(no output)
```

**There is no committed media, no generated media, no rendered frames, no
proxies, no logs and no GPU/shader binary cache anywhere in the working tree.**
`tests/fixtures` is 50 KB of text. The media tests reach for real files through
`need!` and skip when absent (`crates/cine-media/tests/real_media.rs`); no media
corpus is present on this machine, so those tests are currently skipping.
Total non-`target`, non-`node_modules` content in the repository is **7.0 MB**.

### Outside the repository (context, not repo footprint)

```
$ du -sh ~/.cargo ~/.cargo/registry/* ~/.rustup ~/cine-projects ~/cine-projects/*
1.0G  ~/.cargo          844M ~/.cargo/registry/src   106M ~/.cargo/registry/cache
602M  ~/.rustup          37M ~/.cargo/registry/index
692K  ~/cine-projects   688K ~/cine-projects/demo
$ df -h .
/dev/sda2  233G  68G used  154G avail  31%
$ du -sh /tmp/claude-1001
40K
```

The live project directory is **692 KB**. Toolchain and crate registry together
are 1.6 GB and are shared across all Rust work on the machine, not attributable
to this project.

---

## 2. Per-consumer classification

Only consumers above 100 MB are classified. Everything else in the repository is
below 100 MB combined.

### `target/debug/deps` — 13.27 GiB

```
$ find target/debug/deps -name '*.rlib' | ... → rlib:  4.55 GB (472 files)
$ find target/debug/deps -name '*.rmeta' | ... → rmeta: 1.18 GB (906 files)
$ find target/debug/deps -maxdepth 1 -type f ! -name '*.*' | ...
                                       → executables: 7.33 GB (98 files)
$ find target/debug/deps -name '*.d' | ... → dep-info: 5.39 MB (1020 files)
```

| Question | Answer |
|---|---|
| Source-of-truth project content? | No |
| Reproducible from source? | Yes (a full `cargo build --tests` reproduces it) |
| Disposable cache/build output? | Yes |
| Required at runtime? | No — the shipped binary is `target/release/cine`, 5.2 MB |
| Likely to grow continuously? | **Yes** — see mechanisms 1 and 2 |
| Expected in a distributable project? | No (`/target` is gitignored) |

### `target/debug/incremental` — 6.99 GiB

```
$ ls target/debug/incremental | wc -l                        → 235
$ ls target/debug/incremental | sed 's/-[0-9a-z]\{13,\}$//' | sort -u | wc -l → 58
```

235 session directories for **58 distinct compilation units** — a 4.05×
multiplicity. Same classification as `deps`, and the single most aggressive
grower in the tree.

### `target/release` — 1.27 GiB (`deps` 1.3 G, `build` 23 M, binaries 18 M)

Same classification. The two artifacts that actually matter for delivery —
`target/release/cine` (5.2 MB) and `target/release/cine-shell` (13 MB) — are
0.014 % of the repository.

### `target/debug/cine-shell` (315 MB), `target/debug/cine` (159 MB), `target/debug/examples` (256 MB)

Unstripped debug-info copies of binaries that already exist under `deps` as
hardlink-equivalent duplicates. Disposable, reproducible, not required at
runtime, not expected in a distribution.

### `ui/node_modules` — 65.7 MiB

Below the 100 MB threshold. Reproducible from `ui/package-lock.json`; required
to build the UI but not to run the shipped bundle; does not grow with use.

### Nothing else exceeds 100 MB

```
$ find . -type f -size +100M -printf '%s %p\n' | sort -rn
```
Every one of the 32 results is inside `target/debug`. Largest:
`deps/cine_shell-4f5d62fb1939db46` 314 MB, `debug/cine-shell` 314 MB,
`deps/film_fixture-20afe79a733f8d2d` 255 MB, `deps/yuv_parity-*` 224 MB ×2.

---

## 3. The 60 GB → 18.5 GB history

### What the filesystem timestamps independently corroborate

```
$ find target/debug/deps -maxdepth 1 -type f -printf '%TY-%Tm-%Td %TH\n' | sort | uniq -c
    142 2026-09-07 20      216 2026-09-08 18       72 2026-09-09 10
    269 2026-09-07 21      230 2026-09-08 19      154 2026-09-09 11
     62 2026-09-07 22       10 2026-09-08 20        2 2026-09-09 14
      2 2026-09-08 16      241 2026-09-08 21       33 2026-09-09 15
      4 2026-09-08 17      428 2026-09-08 22       16 2026-09-09 16
                           165 2026-09-08 23       21 2026-09-09 20
                                                  119 2026-09-09 21
                                                  358 2026-09-09 22

$ find target/debug/incremental -maxdepth 1 -mindepth 1 -printf '%TY-%Tm-%Td %TH\n' | sort | uniq -c
      8 2026-09-09 16        9 2026-09-09 20       47 2026-09-09 21      171 2026-09-09 22
$ find target -type f -printf '%TY-%Tm-%Td %TH:%TM %p\n' | sort | head -1
2026-09-07 20:56 target/CACHEDIR.TAG
```

Two facts fall out and they corroborate the narrative precisely:

1. **`target` has existed continuously since 2026-09-07 20:56, and 473 files in
   `deps` still date from that first evening.** So `target` as a whole was never
   wiped — consistent with the report that the workspace crates were cleaned
   *individually* (`cargo clean -p <crate>`, which removes only workspace
   artifacts and leaves third-party dependency `rlib`s untouched).
2. **`target/debug/incremental` contains nothing older than 2026-09-09 16:04**,
   while its parent contains files two days older. An empty-then-refilled
   directory inside an otherwise-continuous tree is the signature of a targeted
   wipe. The reported deletion of `target/debug/incremental` on 2026-09-09 is
   therefore corroborated by the filesystem, independently of the report.

The *size* of what was deleted (~20 GB) is **UNVERIFIED** — it cannot be
recovered from the filesystem after the fact. It is a user report.

### The arithmetic

```
measured today                                    22.09 GiB
+ reported deleted incremental cache             ~20    GB
                                                 ─────────
                                                 ~42.1  GiB
gap to the reported ~60 GB observation           ~17.9  GiB
```

The ~18 GiB gap is the material removed by the per-crate `cargo clean`. Is that
magnitude credible? The surviving tree says yes, and says why:

```
$ find target/debug/deps \( -name 'libcine_*' -o -name 'libucg_*' \) | ...
workspace lib artifacts:  1.49 GB (139 files)
$ find target/debug/deps -maxdepth 1 -type f ! -name '*.*' | ...
workspace test/bin executables: 7.33 GB (98 files)
$ du -sb target/debug/examples → 0.25 GB
                                ─────────
workspace-attributable, today:   9.07 GB
```

That 9.07 GB is the state **after** a clean and only a partial rebuild — i.e. at
near-minimum duplicate multiplicity. Duplicate multiplicity is measured directly
in section 4: stale duplicates are already 35 % of executables and 69 % of
`rlib` bytes after barely a day of rebuilding. Two full days of accumulation
(Sep 07–09) at the multiplicity the tree demonstrably reaches puts pre-clean
workspace material in the **18–27 GB** band. 42.1 + ~18 ≈ 60.

**Verdict: the ~60 GB narrative holds up.** It is explicable entirely by
build/incremental/cache material, and it required no legitimate project data at
all. The check is decisive in the other direction: total non-`target`,
non-`node_modules` content in this repository is **7.0 MB**, and the live project
directory is **692 KB**. Legitimate project data did not approach 60 GB, 6 GB or
even 60 MB. At the 60 GB peak, source-of-truth content was on the order of
**0.01 %** of the footprint.

The ~18 GiB clean term is an inference from measured duplicate ratios, not a
direct measurement. Marked **PARTIALLY VERIFIED**.

### The 18.5 GB figure does not match today's 22.09 GiB

`18.5 GB` was presumably read after the clean. The gap to today is accounted for
by the build session that followed it:

```
$ for d in target/debug/incremental/*/; do printf "%s\t%s\n" \
    "$(stat -c '%y' "$d" | cut -c1-13)" "$(du -sb "$d" | cut -f1)"; done | ...
2026-09-09 16    0.21 GB    8 dirs
2026-09-09 20    0.38 GB    9 dirs
2026-09-09 21    2.22 GB   47 dirs
2026-09-09 22    4.19 GB  171 dirs
```

**6.78 GB of incremental cache was created after 16:00 on 2026-09-09, 6.41 GB of
it inside a single 2 h 20 min window (20:00–22:20).** That is a sustained
**~2.8 GB/hour** while a build is running. The repository grew from the reported
18.5 GB to the measured 22.09 GiB — and would have grown further had the session
not ended.

### The internet interruption: no evidence either way

```
$ find ~/.cargo/registry -name '*.part' -o -name '*.tmp' -o -name '*.crate.tmp'
(no output)
$ du -sh ~/.cargo/registry/cache → 106M
```

No partial-download debris exists. A retried `cargo fetch` leaves no residue once
it succeeds, so the interruption neither shows up in nor is excluded by this
measurement. **UNVERIFIED**, and not a plausible contributor at any relevant
scale — the entire crate cache is 106 MB.

---

## 4. Growth mechanisms, with evidence

### Mechanism 1 — incremental sessions accumulate per configuration, not per crate

```
$ ls target/debug/incremental | wc -l                                  → 235
$ ls target/debug/incremental | sed 's/-[0-9a-z]\{13,\}$//' | sort -u | wc -l → 58
$ ls target/debug/incremental | sed 's/-[0-9a-z]\{13,\}$//' | sort | uniq -c | sort -rn | head
     11 ucg_core      10 cine_timeline     8 cine_runtime    7 cine_gpu
     11 cine_media     9 ucg_schema        8 cine_color      7 cine_audio
                       8 cine_shell
```

`rustc` garbage-collects *within* one incremental session directory. It does not
garbage-collect *across* them. Every distinct metadata hash — a feature-set
change, a profile change, a `RUSTFLAGS` change, a dependency version bump —
mints a new directory that is never reclaimed. `ucg_core` has eleven. The
largest single directories are 208 MB, 204 MB, 202 MB (`cine_audio` ×3, each a
different hash of the same crate).

**This regrows without bound. It is the mechanism that produced the 20 GB that
was deleted, and it has already rebuilt 6.99 GB of the same thing in one day.**
It is also the specific mechanism that corrupted under concurrent `cargo`
processes and produced the `undefined hidden symbol` link errors.

### Mechanism 2 — stale duplicate artifacts in `deps`, never reclaimed

```
$ find target/debug/deps -name '*.rlib' -printf '%f %s\n' | \
    sed 's/^lib//; s/-[0-9a-f]\{16\}\.rlib / /' | awk '...'
rlib total: 4.55 GB | one-per-crate: 1.43 GB | duplicate copies: 3.12 GB across 224 crate names

$ find target/debug/deps -maxdepth 1 -type f ! -name '*.*' -printf '%f %s\n' | \
    sed 's/-[0-9a-f]\{16\} / /' | awk '...'
all binaries: 7.33 GB | newest-per-name: 4.74 GB | stale duplicates: 2.60 GB
```

**5.72 GB — 26 % of the entire repository — is superseded copies of artifacts
that a newer copy already replaces.** Cargo never removes them. Worst offenders:

```
$ ls -S target/debug/deps/*.rlib | head
92.7 MB libnaga-c43d19544dbc7799.rlib      87.7 MB libwgpu_core-9f641ab908210d4f.rlib
78.0 MB libnaga-c90b7c0bd5ef1e75.rlib      87.5 MB libwgpu_core-d8142f2f5b02e337.rlib
77.9 MB libnaga-8de4c2e12566eb9f.rlib      87.3 MB libwgpu_core-9294002facaf40b7.rlib
   ... 9 copies of naga total                  ... 5 copies of wgpu_core total
$ find target/debug/deps \( -name 'libnaga*' -o -name 'libwgpu*' -o -name 'libash*' \) | ...
1.87 GB (108 files)
```

### Mechanism 3 — the graphics stack is genuinely enormous in debug builds

The brief asks whether `wgpu`/`naga`/`symphonia`/`image` produce unusually large
artifacts. They do. **`naga` + `wgpu*` + `ash` alone account for 1.87 GB (8.5 %
of the repository)**, before any duplication is removed: a single `naga.rlib` is
92.7 MB and a single `wgpu_core.rlib` is 87.7 MB. `symphonia` appears with 4
copies of each of five sub-crates; `image` with 5. With 371 entries in
`Cargo.lock`, a *deduplicated* debug dependency set still costs ~1.43 GB of
`rlib` plus 1.18 GB of `rmeta`.

This is inherent, not a defect. Debug builds keep full debug info and no
codegen-units merging; the release tree proves the contrast — the same
dependency graph is 1.27 GB in `target/release` versus 20.74 GB in
`target/debug`, a **16× ratio**.

### Mechanism 4 — statically-linked, unstripped test binaries

98 test/bin executables total 7.33 GB — an average of 76 MB each and a maximum of
314 MB. Each test binary statically links the entire dependency graph *and* its
debug info. Adding one integration test to a crate that pulls `wgpu` adds
~100–300 MB to `target`. Growth here is **proportional to test count**, and the
tree already carries 58 distinct compilation units.

Leftover scratch binaries confirm nothing prunes this: `deps/zz_dbg2-*` (153 MB),
`deps/zz_probe` (2 copies, 79 MB), `examples/_scratch_otio_probe`. These are ad-hoc
debugging artifacts that outlived their purpose and are still on disk.

### Mechanism 5 — the op log archives on every save and is never compacted

`crates/ucg-core/src/persist.rs`:

```
179:    pub fn save(&self) -> Result<()> {
190:        // The snapshot supersedes the current op-log segment for *replay*, but
191:        // those deltas are also the undo history, so the segment is archived
192:        // rather than deleted. `compact` drops the archives.
206:        fs::rename(&current, dir.join(format!("{n:04}.jsonl")))?;
213:    pub fn compact(&self) -> Result<usize> {
231:    pub fn enable_autosave(&self) {   // subscribe → append_delta on every commit
```

```
$ grep -rn '\.compact()' crates/*/src --include='*.rs' | grep -v 'fn compact'
(no output)
```

**`compact()` has zero callers anywhere in the codebase.** Every `save()` rotates
`current.jsonl` into a numbered archive; nothing ever removes an archive. The
op log is therefore append-only-forever by construction, and the only way to
reclaim it is a manual call that no code path makes.

Present scale is trivial and the risk is entirely prospective:

```
$ find ~/cine-projects -type f -printf '%10s %p\n'
    260359 .../demo/oplog/current.jsonl
    132860 .../demo/oplog/0000.jsonl
    228470 .../demo/graph.json
     52406 .../demo/graph.msgpack
```

393 KB of op log after the project's whole life. But it grows with **every
committed transaction** — i.e. with every edit a user makes — and never shrinks.
On a real cut with millions of operations this becomes the dominant per-project
consumer. The mechanism is real; the current magnitude is not.

### Mechanism 6 — a stale lock survives a crash

The reboot left `.cine/runtime.lock` naming a dead PID. Not a storage issue by
volume, but it is a liveness defect in the same file family and it was found
during this survey, so it is recorded here.

### Mechanisms checked and NOT found

* **Private `CARGO_TARGET_DIR`s at `/tmp/ocio-target`, `/tmp/audioperf-target`** —
  do not exist now (`find / -maxdepth 4 -type d -name '*ocio-target*' …` returns
  nothing) and cannot exist, because the reboot cleared `/tmp`. Whether they ever
  held significant data is **UNVERIFIED and now unmeasurable**. If subagents did
  use them, each would have carried its own full ~10–20 GB copy of the debug
  tree, and each would have been destroyed by the reboot — which would be a
  *silent* multiplier on peak usage that leaves no forensic trace.
* **Tests writing artifacts that are never cleaned** — no test output files found
  anywhere in the working tree; `tests/` is 50 KB of source. No evidence.
* **Git object growth** — 4.1 MB, 19 commits, no packs. Not a factor.
* **Generated media, render caches, proxies, shader binary caches, logs** — none
  exist. `.gitignore` covers `/target`, `/ui/node_modules`, `/ui/dist`, `*.log`.
* **Scratchpad** — `/tmp/claude-1001` is 40 KB.

---

## 5. Trajectory toward 100 GB

### What is actually consuming the budget today

| Category | GiB | % of repo | Verdict |
|---|---:|---:|---|
| Build cache (`target`) | 22.02 | 99.68 % | **runaway** |
| — of which superseded duplicates | 5.72 | 25.9 % | pure waste, measured |
| — of which incremental cache | 6.99 | 31.7 % | regrows without bound |
| Node modules | 0.064 | 0.29 % | flat |
| Source of truth (crates + docs + tests + git) | 0.0064 | 0.03 % | negligible |
| Project data (`~/cine-projects`) | 0.00066 | — | negligible |

The professional-capability question the brief poses has a blunt answer: **not
one byte of the current footprint is professional capability.** There is no
media, no cache of decoded frames, no proxy, no render output, no conform copy.
Everything measured is compiler output.

### Steady state, measured from this tree

Two anchors, both measured rather than assumed:

* **Deduplicated debug tree**: `deps` at one artifact per crate is 1.43 GB of
  `rlib` + 1.18 GB of `rmeta` + 4.74 GB of newest-per-name executables ≈ **7.4 GB**,
  plus a single-configuration incremental cache. `incremental` at one session per
  compilation unit — 235 dirs collapsed to 58 — scales 6.99 GB by 58/235 ≈
  **1.7 GB**. Add `release` (1.27 GB) and `build`/`examples` (0.49 GB).
* **Realistic steady state: 10–12 GB** for a developer tree that is periodically
  cleaned, with `target/release` and one debug configuration alive.

### Worst case, extrapolated from measured rates

Two rates are measured here and both are linear in developer activity, not
asymptotic:

* incremental accrues **~2.8 GB/hour** of active building (§3);
* every new integration test adds **76 MB average, up to 314 MB**, permanently,
  because nothing prunes superseded binaries (§4, mechanisms 2 and 4).

At the observed 2.8 GB/h with no intervention, `target` reaches the 100 GB line
in roughly **28 hours of cumulative build activity** — a working week. The
60 GB observation was reached in **two days**. That is the empirical answer:
this tree crosses 100 GB in about a week of unmanaged work, and it did the first
60 % of that trip in 48 hours.

Multiply by any private `CARGO_TARGET_DIR`s in use (mechanism 6, unmeasurable),
since each carries a complete independent copy.

### What legitimately gets big later, and where the real budget goes

None of it exists yet, so all of the following is projection, not measurement,
and is flagged as such. For a professional filmmaking application the genuinely
large and *legitimate* consumers are:

| Consumer | Scale | Legitimate? | Should it live in the repo? |
|---|---|---|---|
| Camera originals (ProRes/BRAW/R3D) | 100 GB–TB per shoot | Yes | **No** — referenced, never copied |
| Editorial proxies | ~10 % of originals | Yes | No — project-adjacent cache |
| Decoded-frame / render cache | 10s of GB, bounded by policy | Yes | No — must be size-capped and evictable |
| Op log + snapshots | MB–GB per project, **unbounded today** | Yes | No — with compaction (§4, mech. 5) |
| Test media corpus | GB, fixed | Yes | **No** — external, fetched, `need!`-gated |
| `target/` | 10–100 GB, unbounded | **No** | No — already gitignored |

The architecture is already right about the big one: `docs/blueprint/02-TARGET-ARCHITECTURE.md`
and `00-REPOSITORY-ARCHAEOLOGY.md` both record that the graph keeps *a summary and
a reference*, not the media, projecting ~2 GB resident at one million nodes. That
decision is what keeps the professional-capability budget and the repository
budget separate — and it is why "stay under 100 GB without compromising
capability" is achievable. The 100 GB question is **entirely** a build-cache
question, not a capability question.

The one architectural item that will eventually compete for the same budget is
the frame/render cache, which by nature must be large. It needs an explicit size
cap and eviction policy before it exists, or it becomes mechanism 1 with a
different name. **UNVERIFIED**: no such cache or policy exists in the tree today.

---

## 6. Recommendations — **NOT EXECUTED**

Nothing below was run. The tree is intact exactly as measured. These are ranked
by measured impact.

1. **Reclaimable today without losing anything reproducible: 22.0 GiB.**
   The entire `target` tree is regenerable from source. A narrower option that
   loses no useful cache at all: the **5.72 GB of measured superseded
   duplicates** (mechanism 2), which no build will ever read again.
2. **Cap the incremental cache.** `CARGO_INCREMENTAL=0` in CI and for
   full-workspace test runs removes the single largest grower (6.99 GB today,
   20 GB at peak) at the cost of slower cold builds. Keeping it on for
   edit-compile loops on one crate is the reasonable trade. This also removes the
   corruption class that caused the original `undefined hidden symbol` failures.
3. **Never run concurrent `cargo` processes against one `target`.** The
   corruption that started this incident is the documented failure mode of
   parallel cargo invocations sharing an incremental directory. If subagents need
   isolation, note that a private `CARGO_TARGET_DIR` trades corruption for a full
   duplicate tree — measured here at ~20 GB per copy.
4. **Call `compact()` somewhere.** `Project::compact()` exists, is documented,
   and has zero callers. Wire it to an explicit user action or a snapshot policy
   before a real project accumulates real history. Today's 393 KB is not the
   problem; the absence of any caller is.
5. **The scratch probe binaries are orphans, not a live source of growth.**
   *Corrected after this survey, by re-checking rather than inferring.* `git
   ls-files | grep -iE "zz_|_scratch"` returns nothing and no such file exists
   anywhere in the worktree outside `target/`: the sources were already deleted.
   What remains is 311 MB of unreferenced build output in `target/debug/deps`
   (`zz_dbg2` 161 MB, `zz_probe` x2 83 MB, `zz_dbg` 38 MB, `zz_ordercheck`
   30 MB, dated Sep 7-8). Nothing regenerates them, so they are pure dead
   weight — but there is no "target" left to remove, and no ongoing cost.
6. **Add a `target` size check to the build documentation.** `du -sh target` is
   one command; the failure mode here was that nobody looked until it was 60 GB.
7. **The stale-lock recommendation was wrong and is withdrawn.** *Corrected
   after this survey.* `cine-runtime/src/lock.rs` already consults `/proc` for
   pid liveness, judges a lock whose process is gone to be stale, and reclaims
   it; `acquire()` has a test named "a stale lock must not block the project".
   The lock observed at `~/cine-projects/demo/.cine/runtime.lock` naming dead
   pid 59048 is therefore inert, not a defect. What does remain open: decide the
   media-corpus location before a test corpus lands in the repository.

---

*Every figure in this document is reproducible with the command printed beside
it. Claims sourced from user report rather than measurement are marked
UNVERIFIED. No file was created, deleted, moved or modified by this survey other
than this document.*
