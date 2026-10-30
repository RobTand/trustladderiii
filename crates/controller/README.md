# TL3 controller

`tl3-controller` connects the pure kernel to the Postgres store and upstream
executors. Read [the architecture note](../../docs/architecture.md) for the
transaction and recovery contracts.

All commands use the same TOML configuration as `tl3-config`. This release
supports explicit simulation only. It never loads deployment GitHub or model
credentials.

Both commands use `Config::load`, which checks secret-file metadata, including
the optional store password. Only the store reads its password at runtime.
`tl3 status` uses `Store::open_read_only`. It never migrates or writes and
reports an uninitialized database clearly.

```text
tl3 status --config /absolute/path/to/config.toml
tl3d --config /absolute/path/to/config.toml --simulate /absolute/path/to/fake-records
```

The daemon reads one JSON `cli::Input` per line on stdin. For example:

```json
{"LabelledIssue":{"delivery":"fixture-42","owner":"RobTand","repo":"tl3-sandbox","installation":1,"number":42,"actor":24923402,"label":"tl3"}}
```

The example repository, installation and actor must match the configuration.
Fake receipts live outside the controller process. Keep their directory when
restarting against the same database. `--drain` recovers persisted work and exits
without reading stdin. EOF ends an interactive simulation.

Register a replacement executor with one `Registry::register` call. The five
modules under `src/adapters` each contain a fake. The controller owns interning
and persistence. Executors own authoritative reconciliation and epoch fencing.
`adapters::simulation` builds the fake registry. A test starts
with it and replaces one entry with `register`. The `mixed_registry` scenario
uses the real `tl3-worktrees` client in a temporary repository and delegates
the remaining worktree effects to its fake. The other four upstreams stay fake.

Each adapter ticket extends `Delivery` with the fields its real client needs.
The tracer does not predefine issue text, branch text, or PR text fields.

Run development verification through PrismaBuild from a clean commit:

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --anywhere --cpus 2 --demand mem_gb=6 --priority -10 \
  --env PATH=/home/rob/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
  --env RUSTUP_HOME=/home/rob/.rustup --env CARGO_HOME=/home/rob/.cargo \
  --env CARGO_BUILD_JOBS=2 --env RUST_TEST_THREADS=2 \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  --detach -- bash crates/controller/tools/verify.sh
```

Run this from the repository root. Use `pbwait.py --endings KEY` and inspect the
exit code, logs and verified CAS receipt. CI runs separately on Mergify runners.

`adapters::simulation_with_workers` replaces ingest and audit with Python worker
processes and returns a `WorkerOutputSource`. Other roles remain on the worker
fake. Supply issue and repository text through `Store::put_worker_materials`
before dispatch. Audit reads the stored ingest reply. An ingest revision reads
the audit's objections. Missing materials block dispatch.

Run `WorkerOutputSource::run(sender)` on its own ingress thread. It emits
`cli::Input::Observation` and writes progress to a separate store table. Until
#10 supplies the shared daemon loop, simulations can call `poll_once` and feed
its channel to `Controller::observe`. The stdin-only daemon keeps its existing
fake registry until that loop can register this source.

Each effect key owns a directory of immutable attempts. stdout and stderr are
files. A Python supervisor records its identity before launching the worker,
waits for it, fsyncs output and publishes its exit status. The worker wrapper
records its identity before exec. Atomic claims also let recovery cancel a
launcher that has not started. No file locks are used. Machine ID, PID, process start time
and boot ID identify each process. A record from another machine stays unknown. Stop uses pidfds and waits for termination.
Keep these runs on the controller's host, including after a restart.
For #19, workers and their supervisors must live outside the controller's kill
scope. Use systemd `KillMode=process` for the controller or put workers in their
own scope. Otherwise stopping the controller kills the workers too.

Sync the worker environment once at deployment with
`uv sync --locked --project /srv/tl3/workers --python 3.14+gil`.
Set `workers.python` to its `.venv/bin/python`. Each launch uses that interpreter
directly, without uv cache or venv locks. `workers.command` contains Python's
arguments, normally `-m tl3_workers run`.

The source watches only active runs with inotify for output and exit records,
and pidfds for process exits. A durable per-key notification registers a new
attempt. Completed runs leave the watched set. Startup reads the store watches
and run directories once to rebuild it. Stop indexes active runs by node.

Only a valid role reply followed by exit zero is accepted. A recorded failed
exit reports `RunFailed`. A lost supervisor record is absent only after both
identities are proven dead and delayed launches are fenced. The controller
reports absence of an acknowledged worker as `RunFailed`; an uncertain start
can retry the same key. The source emits only `Found` evidence, never `Absent`
or a cancelled claim. The controller watch owns confirmed absence.
If a supervisor dies while its worker lives, recovery waits for that worker.
A valid reply without a recorded exit is discarded, and the run is redone.
This is the safe path because nobody proved a successful process exit.
No clock or liveness threshold makes this decision.

`Store::worker_progress` reads the last progress for every run, optionally
filtered by request node. `Store::worker_document` reads briefs, goals and audit
replies. Replaying output does not refresh progress time. These reads do not
claim controller ownership. `tl3_worker_progress` records `run_key`, `node_id`,
`epoch`, `attempt`, `started_ms`, `output_offset`, `unit`, and nullable
`last_progress_ms`. A start row exists before the first progress event.
Replaying a claim or output does not refresh its timestamp. Ticket #15 can
measure silence from `started_ms`. Ticket #18's status hook receives a
`Transaction` and queries this table directly in its read-only snapshot.

Submit `bash crates/controller/tools/verify-workers.sh` through PrismaBuild to
run the process acceptance scenarios. They run the actual worker entry point
with a fake model and deny network calls. They kill workers and controllers
with SIGKILL, check reattachment, exercise invalid and premature replies, and
verify the goals delivered to the GitHub fake.

For the review-fix evidence, create a clean detached worktree at the final
commit, then generate the campaign there:

```bash
python3 crates/controller/tools/review_manifest.py > /tmp/tl3-review-campaign.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-review-campaign.json
```

The campaign adds the kernel tests and three Kani harnesses covering retryable
acknowledgements, publishing transitions, and queued progress. Read each terminal
record, log, and CAS receipt before reporting a pass.

## Add a worker role

For #11, #12 and #13, update each boundary together:

- Add the kernel role mapping in `adapters/workers/protocol.rs` `role`.
- Map and validate the reply in `map_reply`, using the package's role schema.
- Validate role inputs in `validate_inputs`.
- Build inputs and select the model ladder in `Supervisor::request`.
- Extend `lib.rs` `valid_role` and the document names in `translate`.
- Add process scenarios for accepted replies, invalid output, recovery and stop.

Ticket #11 replaces `IngestDone { size: 1 }`. The forecaster owns sizing.
Until these boundaries support a role, its starts and stops use the worker fake.

Worker tests resolve `TL3_TEST_UV` first, then `uv` on PATH. Missing uv fails
with an installation message. They sync the locked Python 3.14 environment once
per test process. Run the process scenarios on dl380g10, sparky and sparklina
through PrismaBuild with `--tag HOST`. Use `worker_review_manifest.py` to generate
the three host actions and the portable full check from a clean detached worktree.
