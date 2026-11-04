# Worker supervisor protocol and model

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
be installed with the controller binaries before new attempts launch.

The old executable is frozen under `tests/worker_supervisor/legacy.py` solely as
a compatibility fixture. No production path embeds or launches it.

## P12 and S4 model

`tests/worker_supervisor/model.rs` models two generations, each with two competing
supervisors and one worker. The finite bounds include one controller crash and
restart, one supervisor loss at any active stage, and one old-generation replay.
These bounds exercise each identity relationship and both winners of a duplicate
claim. They do not establish an unbounded proof over arbitrary crashes or runs.

The checker exhausts the reachable graph without a depth or state-count cutoff.
Safety properties require one exit writer per claim, at most one accepted outcome
per generation, and an exit only after reaping. Completion requires both runs to
be inspected and every launched child to be reaped. Witnesses require a lost exit,
a controller crash overlapping worker exit, and two successful generations.

A missing exit is absence, as in the existing controller. It is never synthesized
as success. Reconciliation can start another attempt using the existing policy.
Exactly-once publication means a successful owner publishes one exit, duplicates
publish none, and a lost publication remains absent. It does not mean the original
worker's exit code can be recovered after that record is lost.

The model assumes atomic filesystem link and rename, successful fsync durability,
and eventual OS scheduling and child exit. After an owner dies, Linux adopts and
reaps its child. `ReapOrphan` models that OS guarantee, rather than claiming the
supervisor can waitpid a child after it dies. Disk corruption, machine loss, PID
namespace changes, and unbounded worker execution are outside the model.

The conformance test observes real claim and start files, disconnects the
controller handshake, races a duplicate helper, waits for exit, and then starts a
new generation. It replays those events through the model. It runs once with a
Rust owner whose executable is unlinked before fork, and once with a live legacy
Python owner and a Rust contender. FIFO EOF and inotify order the test without
sleep-based synchronization.

The gated mutant check permits a rejected supervisor to publish an exit. It must
produce a counterexample. The migration's red commit runs the exhaustive property
check with this mutant enabled and runs the compatibility fixture against a stub.

## Transition map

All production paths below are relative to `crates/controller/`.

| Model action | Production operation |
| --- | --- |
| `Claim` | `src/bin/tl3-worker-supervisor.rs:43`, exclusive `claim.json` link at line 124 |
| `Spawn` | `src/bin/tl3-worker-supervisor.rs:58`, fork the pinned helper image |
| `Start` | `src/bin/tl3-worker-supervisor.rs:26`, exclusive `process.json` link before exec at line 41 |
| `Exit` | OS worker exit observed by `wait` at `src/bin/tl3-worker-supervisor.rs:65` |
| `Reap` | `src/bin/tl3-worker-supervisor.rs:65`, `Child::wait` reaps before output fsync |
| `ReapOrphan` | Linux child adoption after owner loss, an external assumption |
| `Publish` | `src/bin/tl3-worker-supervisor.rs:71`, exit publication through rename at line 130 |
| `Lose` | Process loss between operations in `src/bin/tl3-worker-supervisor.rs:43` through line 71 |
| `Inspect` | `src/adapters/workers.rs:1441`, cancellation links, identity checks, and exit read |
| `CrashController` | Loss of the controller while its independently scoped supervisor keeps running, `src/adapters/workers.rs:1111` |
| `RestartController` | Reconciliation through `src/adapters/workers.rs:1428` |
| `Advance` | New delivery generation and separate attempt directory, `src/adapters/workers.rs:1073` |
| `Replay` | Generation refusal in `src/lib.rs:3072` and the kernel's existing reply-generation guard |

`//crates/controller:test_test_worker_supervisor` gates the model, mutant check,
byte-compatibility fixture, cancellation check, and real-trace conformance test.
Existing worker scenarios remain gated by `//crates/controller:test_test_workers`.
No test assertions are pruned in this migration. The late-launcher scenario now
invokes the Rust helper instead of staging the production Python source.
