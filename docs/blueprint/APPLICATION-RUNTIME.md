# The Application Runtime

**Canonical command:**

```bash
cine start
```

Run from a project directory. That is the whole of it — there is no separate
backend, bridge, UI server or media service to start.

```bash
cine start                       # the project in the current directory
cine start /path/to/project      # a project elsewhere
cine start --port 8080           # a different port
cine start --no-ui               # API only
cine start --isolated            # a second runtime, deliberately (see §3)
```

---

## 1. What it does, in order

```
[1/5] opening project      one OmniProject, one Engine, one op log
[2/5] locating UI          ui/dist, or a clear message if unbuilt
[3/5] binding runtime      preferred port, or an OS-assigned one if busy
[4/5] taking the lock      one runtime per project directory
[5/5] starting             change broadcaster, signal handler, accept loop
```

It then prints the address actually bound — not the address requested, which
may differ — and serves until Ctrl-C, at which point it saves the project,
releases the lock and exits.

---

## 2. The architecture, and the defect it replaced

**Before.** A Node bridge spawned one `cine serve` process per browser
connection. Every window therefore opened the project independently:

```
window A ──ws──▶ bridge ──spawn──▶ cine serve ──▶ engine A ──▶ op log
window B ──ws──▶ bridge ──spawn──▶ cine serve ──▶ engine B ──▶ op log
```

Two windows meant two engines, two op logs, and two divergent notions of the
truth with no mechanism that would ever reconcile them. Whichever saved last
won and the other window's work was gone. Nothing warned anybody.

**Now.** The runtime owns the project; clients are views onto it.

```
                        cine start
                            │
                    ┌───────┴────────┐
                    │    Runtime     │   one OmniProject, one Engine
                    └───────┬────────┘
                ┌───────────┼───────────┐
             window      window        CLI
                └───────────┼───────────┘
                            │
                      same UCG state
```

A commit by any client is broadcast to every client, including the one that
made it, because the engine's revision is the shared truth and a client
tracking its own optimistic state would drift from it.

**Verified**, not asserted: `two_clients_share_one_project_and_never_diverge`
opens two real WebSocket connections against the real server, has one client
edit and the other undo, and fails if either the notifications or the revisions
diverge.

### Why not async

The rest of the workspace is synchronous — `parking_lot`, `crossbeam-channel`,
threads, no `async` anywhere. Adopting an async stack would have introduced a
runtime and two colours of function across a codebase that has neither, to
serve a handful of local windows. Thread-per-client is the right shape at this
scale, and at that point the WebSocket protocol itself is a few hundred lines.

New dependencies, all MIT/Apache-2.0: `sha1` and `base64` (the whole of RFC
6455's cryptographic surface) and `ctrlc`.

---

## 3. A second terminal in the same directory

Two runtimes on one project would each hold an engine and each append to the op
log, interleaving two incompatible histories into a file that could then never
be replayed. That is silent corruption of the entire project.

So a second `cine start` in the same directory **attaches rather than
competes**: it reads the instance lock, reports where the running runtime is,
and exits successfully.

```
$ cine start
CINE-OMNI is already running for this project.
  pid     47081
  open    http://127.0.0.1:7331/
```

Opening a second window against that address shares its state — that is the
supported way to get two windows.

`--isolated` starts a second runtime anyway. It is deliberately explicit, takes
no lock, and must not be pointed at a project another runtime owns.

### Stale locks

A crash leaves a lock behind. The lock records the pid, and a lock whose
process is gone is stale and is reclaimed, so a crash never makes a project
permanently unopenable. A *corrupt* lock is refused rather than assumed free,
because it may belong to a live runtime whose file was damaged — and guessing
wrong there is the corruption this exists to prevent.

---

## 4. Endpoints

| Path | Purpose |
|---|---|
| `/` and static paths | The built UI from `ui/dist` |
| `/ws` | JSON-RPC 2.0 over WebSocket — the application API |
| `/health` | Runtime state: clients, revision, project, uptime |

Bound to **loopback only**. A film project is not a public web service, and
nothing here authenticates.

The UI derives its socket address from the page's own origin rather than a
configured default, so it follows the runtime if the preferred port was busy.

---

## 5. One dispatch, two transports

`cine-runtime::rpc` is the only JSON-RPC implementation. Both `cine start`
(WebSocket) and `cine serve` (stdio, single client, for scripting) dispatch
through it, so a method cannot exist on one transport and not the other, or
behave differently depending on how it was called.

---

## 6. Lifecycle

| Concern | Behaviour |
|---|---|
| Port collision | Falls back to an OS-assigned port and reports it |
| Ctrl-C / SIGTERM | Orderly: stop accepting, join the broadcaster, save, release the lock |
| Save on exit | Automatic; a failure is reported loudly rather than swallowed |
| Client that stops reading | Dropped after its bounded queue fills, rather than growing memory without limit |
| Connection panic | Contained to that connection's thread |
| Exit codes | 0 on clean shutdown or attach; 2 on a usage or startup error |
| Unbuilt UI | Serves the API and says how to build the interface |

---

## 6a. One application: the native surface

When there is a display, `cine start` opens the native window in the same
process. It is a surface of the running project, not a second application:

```
cine start
  ├─ runtime + protocol + browser surface      (server thread)
  └─ native window                             (main thread; winit requires it)
       ├─ reads      playlist::edit_list        the same flattened cut the
       │                                        browser's record monitor reads
       ├─ plays      EditListSource → DecodeAhead → Scheduler → planar GPU path
       ├─ follows    engine commits             reloads the cut, keeps the
       │                                        playhead, resumes if playing;
       │                                        commits that do not change the
       │                                        cut do not interrupt playback
       └─ sends      track state                through the graph, as the
                                                protocol does
```

Closing the window closes the application; Ctrl-C closes the window. Both end in
the same orderly shutdown: stop accepting, save, release the lock. With no
display, or `--no-window`, the application is the runtime and the browser
surface, and the start banner says which.

Measured on the reference laptop (release, integrated GPU), playing a
two-clip sequence built from real media through the protocol, with the
workspace drawn (`cine start --window-play 4`):

```
transport Playing — presented 96, dropped 0, late 0
preroll: 25 pictures in 2.32 s
decode-ahead: 96 decoded (mean 62.6 ms, worst 978 ms), peak 39.6 MiB
stages: planes to GPU 0.76 ms per picture; draw incl. conversion 6.4 ms
underruns: 1 episode
```

The worst decode is the cold entry into the second file (a keyframe walk);
decode-ahead absorbed it. A 4-second sequence of two 2-second clips is a short
test, and the number claims only that.

## 7. Status

| Capability | State |
|---|---|
| One-command startup | 🟢 VERIFIED — `cine start` from a fresh terminal |
| Second terminal attaches safely | 🟢 VERIFIED |
| One runtime, many clients, no divergence | 🟢 VERIFIED — over real sockets |
| Orderly shutdown, save, lock release | 🟢 VERIFIED |
| Stale-lock reclamation | 🟢 VERIFIED |
| Port fallback | 🟢 VERIFIED |
| Path-traversal refusal on static files | 🟢 VERIFIED — including symlink escape |
| Crash recovery | 🟢 VERIFIED — see §8 |
| Native surface in the same process | 🟢 VERIFIED — plays the project's sequence, reloads on commit, closes cleanly on Ctrl-C |
| Which sequence each surface has open, shared | 🔴 NOT IMPLEMENTED — the window plays the most recently created sequence |
| Authentication / authorisation | 🔴 NOT IMPLEMENTED — loopback only is the whole of the security model |
| Reconnect with state resync | 🔴 NOT IMPLEMENTED — a client that drops must reload |
| Per-client permissions or roles | 🔴 NOT IMPLEMENTED |
| Conflict resolution between clients | 🔴 NOT IMPLEMENTED — last write wins within one engine, which is coherent but not collaborative |
| TLS | 🔴 NOT IMPLEMENTED |

---

## 8. Crash recovery

Every commit is appended to the op log as it happens, so durability does not
depend on anyone remembering to save. A snapshot is written on a clean
shutdown; between snapshots the log is the record.

**Tested by actually killing the process.** A crash test that calls a shutdown
function tests the shutdown function. `crates/cine-runtime/tests/crash_recovery.rs`
spawns the real binary, edits a project through its WebSocket API *without
saving*, sends `SIGKILL` — uncatchable, so no handler runs and no snapshot is
written — and starts it again. It first asserts the op log is newer than the
snapshot, so a run where the edits happened to have been saved cannot pass
vacuously.

| Guarantee | State |
|---|---|
| Committed edits survive a `SIGKILL` | 🟢 VERIFIED — recovered from the op log alone |
| A lock the dead process could not release is reclaimed | 🟢 VERIFIED — a crash never makes a project permanently unopenable |
| A record torn by an interrupted write is discarded | 🟢 VERIFIED |
| …and **reported**, not silently swallowed | 🟢 VERIFIED |

The report matters. Recovery used to discard a partial record without a word,
which tells the user nothing happened when something did. Startup now says:

```
      recovered 11 operation(s) from the op log
      NOTE: the previous session ended mid-write. A partial record
            of 114 bytes was discarded -- it was never a completed
            transaction, so nothing committed has been lost.
```

The distinction the recovery makes is deliberate: a **torn tail** is the last
line of the last segment, which is what an interrupted append leaves, and
discarding it loses nothing that was ever committed. Corruption *anywhere else*
has valid records after it, which means something other than a crash, and that
fails loudly instead.

### What is not covered

`append_delta` writes without `fsync`. A killed process cannot lose data — the
bytes are already in the page cache and the kernel outlives it — but a **power
failure or kernel panic** can lose recent commits. That is a different
guarantee, needs `sync_all` per append, and would cost a disk round trip on
every edit. Not done, and not claimed.

Autosave is currently "append every commit", which is durable but never
compacts until a clean save. A long session without one grows the log
indefinitely. Checkpoint policy is 🔵 PLANNED.
