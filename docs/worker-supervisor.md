# Worker supervisor protocol

`tl3-worker-supervisor` is a binary in `tl3-controller`. Release packaging installs
it beside `tl3` and `tl3d`. The controller passes the attempt directory to the
helper through the configured scope launcher. The helper forks itself with
`--worker`, then that child execs the Python runtime pinned in `launch.json`.
`/proc/self/exe` preserves the helper's running image if a deploy replaces its
installed file before the fork.

## On-disk compatibility

The v1 protocol is unchanged. `request.json`, `command.json`, and `launch.json`
remain controller inputs. `claim.json` and `process.json` retain the ordered
`pid`, `machine`, `start`, and `boot` fields. Cancellation retains
`{"cancelled": true}`. The handshake is `claimed\n`. Start and exit records retain
`{"started_ms": N}` and `{"code": N, "finished_ms": N}`. A signal exit has the
negative signal number. JSON separators include one space, with no final newline.

Publications write and fsync a new `NAME.UUID4HEX` file, then use a hard link for
an exclusive claim or rename for a replaceable record, then fsync the directory.
The worker's stdin reads `request.json`, stdout truncates `stdout.jsonl`, and
stderr appends to `stderr.log`. The owner waits for and reaps the worker before
fsyncing output and publishing its exit.

No drain of in-flight workers is required for this migration. Old supervisors
continue executing their staged Python file and pinned release runtime. The new
controller reads the same identities and exit records when it reattaches. Old
controllers can also read records produced by the Rust helper. The helper must
be installed with the controller binaries before new attempts launch. TL3 lead:
install the complete four-binary bundle before restarting; keep legacy attempt
files and pinned runtimes. The bundle is `tl3`, `tl3d`, `tl3-deploy`, and
`tl3-worker-supervisor`.

The old executable is frozen under `tests/worker_supervisor/legacy.py` solely as
a compatibility fixture. No production path embeds or launches it.

## Trace conformance

`//crates/controller:test_test_worker_supervisor` runs the legacy byte fixture,
cancellation checks, and one real-trace conformance test. The process trace starts a
supervisor, disconnects the controller handshake, races a duplicate, and waits
for exit. It runs with a Rust owner whose executable is
unlinked before fork, and with a live legacy Python owner and a Rust contender.
FIFO EOF and inotify order the test without sleep-based synchronization.

The test replays the real attempt's claim, process, start, and exit records into
a fresh directory. A duplicate must preserve every recorded byte and must not
start another worker. Removing the replayed exit record must leave it absent,
without re-executing the worker.
A separate controller test starts a live legacy-owned attempt with its staged
Python script, reopens the persisted controller with the Rust launch path,
reconciles the live attempt, and cancels it through liveness recovery. A pidfd
confirms the old worker terminated before the next generation starts. Delivering
the legacy worker's recorded reply then leaves the newer world and watch intact.
The compatibility fixture also checks signal status and reaping through the
real helper. A differential descriptor fixture verifies both implementations
close an inherited non-CLOEXEC sentinel descriptor at the worker boundary. These tests exercise concrete traces; they are not an exhaustive
protocol proof.

Existing worker scenarios remain gated by
`//crates/controller:test_test_workers`. Their assertions remain intact. The
late-launcher scenario invokes the Rust helper instead of staging the production
Python source.

## Descriptor boundary

The owner marks all descriptors above stderr close-on-exec in a `pre_exec`
hook. This matches Python's default `close_fds=True`. `CLOSE_RANGE_CLOEXEC`
keeps Rust's spawn-error pipe usable until exec. The unsafe block runs only
in the forked child and calls `close_range`, without allocation or locks.
A syscall error fails the spawn instead of letting descriptors escape. This
uses Linux's `CLOSE_RANGE_CLOEXEC` support, available since Linux 5.11.

## Verus follow-up

The protocol proof moves to a separate Verus follow-up under #730. No protocol
model-checking framework or proof dependency ships in this change. The follow-up
needs to check these properties under worker exit racing controller restart,
duplicate supervisors, lost exit records, and two contenders for one claim:

| Property | Production transitions |
| --- | --- |
| At most one supervisor owns an attempt, and a rejected contender starts no worker and publishes no outcome | `crates/controller/src/bin/tl3-worker-supervisor.rs:43`, exclusive claim; `:137`, atomic link; `:77`, child spawn; `:84`, exit publication |
| At most one worker executes for an attempt, including cancellation before its wrapper starts | `crates/controller/src/bin/tl3-worker-supervisor.rs:26`, process claim; `:41`, exec; `crates/controller/src/adapters/workers.rs:1444` and `:1449`, cancellation fences |
| A surviving owner publishes exactly one exit after worker exit and reaping; duplicates cannot overwrite it | `crates/controller/src/bin/tl3-worker-supervisor.rs:78`, wait/reap; `:79`, output fsync; `:84`, exit publication; `:143`, rename |
| A lost exit remains absence, never fabricated success; a live owner or worker prevents absence | `crates/controller/src/adapters/workers.rs:1445`, liveness checks; `:1453`, exit read and absent result |
| Restart preserves the independently scoped owner and its recorded identity, including a worker exit during disconnection | `crates/controller/src/adapters/workers.rs:1114`, launcher setup; `:1428`, recorded reconciliation; `crates/controller/src/bin/tl3-worker-supervisor.rs:48`, handshake; `:78`, wait/reap |
| An outcome is accepted at most once for its operation, and an older generation cannot affect a newer one | `crates/controller/src/adapters/workers.rs:1073`, attempt selection; `:1127`, pinned launch record; `crates/controller/src/lib.rs:3072`, reply-generation guard |
| Every launched child is eventually reaped | `crates/controller/src/bin/tl3-worker-supervisor.rs:78`, owner wait; Linux adoption and reaping after owner death is an explicit external assumption |

The proof must state atomic link/rename, successful fsync durability, eventual
scheduling and worker exit, and Linux orphan reaping as assumptions. Disk
corruption, machine loss, PID namespace changes, and unbounded worker execution
need explicit treatment or exclusions. Exactly-once publication cannot recover a
worker's original exit code after its owner dies without publishing it. The
existing controller may reconcile that absence and retry under its current rules.
