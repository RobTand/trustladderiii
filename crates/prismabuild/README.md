# tl3-prismabuild

Rust edition 2024 client for ticket #30, in the root Cargo workspace.

`PrismaBuild::new(Options)` takes an absolute queue path, a submission tool and
the numeric HTTP origin of the bell for that queue. Production uses
`Tool::published()`, `/mnt/shared/prismabuild-fleet/pb-queue`, and the queue
host's bell on port 9470. There are no implicit fleet connections in tests.
The published tool owns its fleet root. A different queue requires a matching
tool adapter as well as a different bell endpoint.

## Operations

- `submit(&SubmitAction)` runs `pbrun` detached on the pool and returns the full
  action key, arrival status and publication timestamp. It supplies CPU and
  memory demand, GPU demand, a pinned container image, progress phases and an
  optional progress cycle. Memory demand is a positive unsigned integer in GiB.
  It requests portable placement and one attempt.
  It never supplies `--timeout-s`. The action must report cumulative committed
  units through PB's progress helper. The caller bounds its processes and
  threads within the declared demand. Native library threads default to one.
  Caller environment keys starting with `PRISMABUILD_ACTION_PROGRESS_` are
  rejected because PB owns the progress channel.
- `read(&ActionKey)` returns the current queue snapshot or `None` for an absent
  key. It maps ready, claimed, done, failed, lease-lost and withdrawn records.
  A failed job keeps its PB status and action return code. `action_returncode`
  takes precedence over the launcher's `returncode`. An unknown terminal
  status in `done` is an error. A cache-hit record in `done` finishes with zero.
- `withdraw(&ActionKey, reason)` invokes `pbrun --withdraw --json`. The response
  records the cancellation decision. It does not prove that the process or
  its cleanup has finished. Finished, failed, lost or unknown work confirms
  immediately without invoking the tool, as the state table requires.
  Exit 2 triggers another authoritative read. It confirms only if that read
  finds the job absent or terminal. Other nonzero tool exits remain errors.
- `reconcile(&BTreeSet<ActionKey>)` rereads every awaited key after a restart,
  including keys that currently have no queue record. It returns a result per
  key, so one unreadable or malformed record does not hide the others. It never
  submits work.
- `watcher().next(awaited, patience)` captures `/v1/now` before the initial
  record reads, then long-polls `/v1/wait` with the awaited action keys in the
  `keys` filter. Ordinary replies reread only hinted awaited keys. Empty replies
  return no observations. Resets, transport failures and changes to the awaited
  set rescan every awaited key. Record errors remain per-key results and do not
  stop cursor advancement. The caller retries failed reads with `read` or
  `reconcile`. Recreate the watcher after a controller restart.

The watcher returns `bell_available: false` and rescans when the bell fails or
returns invalid data. Repeated immediate replies and failures back off from
250 ms to five seconds. Transport requests are bounded to at most 300 seconds.
The controller owns calling `next`, cancellation by dropping its future, and
persisting observations. Bell messages are hints, never action outcomes.
The bell's cap of 128 applies to simultaneous connections, not action keys.
An empty awaited set captures `/v1/now` without opening an unfiltered wait.

## Evidence and recovery

The newest publication wins when an old terminal record overlaps new work.
Within a publication, withdrawal wins over completion, then completion over a
claim and a claim over ready. These reads are observations across files,
not a transaction. A later observation can supersede an earlier one.

Linked immutable attempt records preserve prior failures and lease loss,
including history inside `withdrawn_attempt` after a withdrawal. `Queued` and `Claimed` describe
the current attempt even when a previous attempt lost its lease.
`LeaseLost` requires PB's `lease_lost` or `lease_lost_max_attempts` terminal
record. A missing or old lease never causes this client to declare work lost.
Claimed snapshots expose the lease heartbeat and progress observation for looks.
PB #2426 does not yet publish renewals on the bell.

Records and attempts retain only the return code, termination reason, the last
8192 UTF-8 bytes of stderr, and typed progress counters and timestamps. The tail
starts at a character boundary. Broker argv, tokens, nonces, resource telemetry
and unknown progress fields are discarded. Lease progress uses the same typed
observation. These objects never retain the raw detail JSON.

Unreadable or malformed evidence returns an error. A missing queue directory
also returns an error. An absent key is not a failed action or permission to
start another copy. Cache hits are reported directly by `submit`; this client
does not validate CAS receipts or infer completion from a missing queue record.
The caller must retain the submission outcome when no new publication exists.
If `pbrun` stdout cannot be parsed, resubmit the identical action. Resubmission
is safe: PB attaches to the identical action or serves its cached result. The
client returns the parse error and leaves that decision to the caller.

TL3 deliberately submits with `--max-attempts 1`. Its kernel owns retries so
every failure counts toward an incident under the state table's "Failures"
section. A lease loss therefore ends the action. TL3 retries in a new generation.

Queue reads can block on the shared filesystem. Bell transport limits do not
bound an NFS read or the submission process. The controller owns isolation of
those operations and its durable effect sequencing.

## Contracts and verification

The contracts come from the published PB skill, `docs/operating_prismabuild.md`,
`tools/fleet/pbrun.py`, and the `pool`, `lifecycle`, `record_store`, `action_client`,
`submission_frontend` and `queue_bell` source modules, read on 2026-10-30.
The queue and attempt schemas retain PB's `prismaquant.prismabuild` prefix.
Detached submissions accept both the published `prismabuild.submission.v1`
schema and the prior `prismaquant.prismabuild.pbrun_detach.v1` schema.

Tests create private temporary queue directories, invoke only
`tests/fixtures/fake_pbrun.py`, and use an in-process HTTP bell. No test submits,
withdraws or reads real fleet actions. The fake records literal argv and emits
caller-selected JSON and exit codes. Queue fixtures reproduce the source
layouts, including generation-scoped attempt links and withdrawal history.
`tests/fixtures/queue` contains real done, failed and withdrawn records and their
linked attempts, captured on 2026-10-30. Tokens and nonces use fixed fake values.
To refresh them, run `python3 tests/fixtures/capture_queue.py --root QUEUE_ROOT`
from this crate. The script copies only its explicitly listed action keys.

From a clean commit, submit `sh crates/prismabuild/verify.sh` through the
published `pbrun.py` with `--anywhere --cpus 2 --demand mem_gb=4` and these
environment settings:

```text
PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin
RUSTUP_HOME=/home/rob/.rustup
CARGO_HOME=/home/rob/.cargo
KANI_HOME=/home/rob/.kani
CARGO_BUILD_JOBS=2
RUST_TEST_THREADS=2
OMP_NUM_THREADS=1
MKL_NUM_THREADS=1
OPENBLAS_NUM_THREADS=1
```

Check `pbwait.py`, the terminal record, logs and CAS receipt before reporting
success. CI runs on Mergify runners, outside PrismaBuild submission.

The controller adapter adds `seal(&SubmitAction)`, `seal_at_priority` and
`publish(&ActionKey)`. Sealing calls PB's `--seal-only` and publishes no runnable
work. Persist its key durably before calling `publish`, which uses only
`--publish-sealed KEY --detach`. This reuses the retained CAS request across a
crash or runtime update. Publication validates that PB returned the requested
key. Priority is publication policy, outside action identity; the controller
uses -10 for its development trial. The ordinary `submit` interface is unchanged.

Tool children set `PR_SET_PDEATHSIG=SIGKILL` before exec and check their parent
PID afterward. The spawning thread must live until the tool finishes. Linux
delivers the death signal when that thread exits, even if the process remains
alive. The controller adapter spawns on its daemon owner thread through a
current-thread runtime. This prevents a publisher from surviving controller
SIGKILL and landing work after recovery has established absence.
