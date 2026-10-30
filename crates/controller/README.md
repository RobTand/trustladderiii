# TL3 controller

`tl3-controller` connects the pure kernel to the Postgres store and upstream
executors. Read [the architecture note](../../docs/architecture.md) for the
transaction and recovery contracts.

All commands use the same TOML configuration as `tl3-config`. Controller execution
supports explicit simulation only. Status commands read the configured store
and do not load GitHub or model credentials.

Both commands use `Config::load`, which checks secret-file metadata, including
the optional store password. Only the store reads its password at runtime.
`tl3 status` uses `Reader::open`. It never migrates or writes and
reports an uninitialized database clearly.

```text
tl3 status --config /absolute/path/to/config.toml
tl3 status-serve --config /absolute/path/to/config.toml
tl3d --config /absolute/path/to/config.toml --simulate /absolute/path/to/fake-records
```

The daemon reads one JSON `cli::Input` per line on stdin. For example:

```json
{"LabelledIssue":{"delivery":"fixture-42","owner":"RobTand","repo":"tl3-sandbox","installation":1,"number":42,"title":"Fix the sandbox build","actor":24923402,"label":"tl3"}}
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

`tl3 status` and the page share `status::load`, one typed model and one set
of columns. They show the issue title and number, phase, wait, credit, last
progress and open incidents. Older requests without a title show "title not
recorded". Both progress and incidents show "none recorded" until their store
data exists. Tickets #9 and #15 fill `status::load_request_details`. It takes all
node IDs at once and returns a map from node ID to `RequestDetails`, using batch
queries on the existing read-only transaction. Missing entries use the empty
details. Return public summaries there, never
worker payloads, connection settings or tokens.

`tl3 status-serve` is a separate process from `tl3d`. It serves `/` with GET
and HEAD, needs no login and refuses other methods with 405. Reload the page
to refresh it. The server uses `config.status.listen`; the example binds only
to `127.0.0.1:8033`. Initialize the database through the controller before
opening status. Status checks the schema and migration version before reading;
an empty database reports "database not initialized". Status never runs migrations
or creates request data. After a connection failure, the page reopens its single
reader and retries once. Failed reads return a generic 503.

The synchronous HTTP server uses four handler threads and caps accepted open
connections at eight, including queued clients. Extra connections close immediately.
Reading headers and writing each response have separate two-second total deadlines.
Each socket requests a 32 KiB send buffer to bound queued replies.
Response writes use chunks and the remaining deadline, so trickle readers cannot
keep a handler indefinitely. Accept errors are logged and skipped; file-table
exhaustion also backs off briefly. Each 503 logs its cause to stderr without
including it in the HTTP body. Queries on `/` are ignored. Request lines plus headers are
limited to 8 KiB. Request bodies are rejected and every response closes its
connection. Responses keep the content security policy and disable caching.

Ticket #19 owns deployment on dl380g10, the systemd unit and the LAN address.
The page has no authentication. Run the status process in its own unit and set
`[status].listen` to the LAN interface's specific address, never `0.0.0.0` or
`[::]`. The status connection uses the `[store]` settings. A SELECT-only
Postgres role can also restrict the status process's credentials.

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

`adapters::simulation_with_workers` replaces ingest, audit and planning with Python worker
processes and returns a `WorkerOutputSource`. Making and review remain on the worker fake. Supply issue and repository text through `Store::put_worker_materials`
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
filtered by request node. `Store::worker_document` reads briefs, goals, audit
replies, plans and findings. Replaying output does not refresh progress time. These reads do not
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

The real checks adapter lives in `adapters::prismabuild::Adapter`. Register it at
`Upstream::PrismaBuild`. Its `source().spawn(sender)` starts the ingress thread;
keep the returned handle alive. Its `join()` returns source errors or panics to
the daemon supervisor, which must stop the ingress loop and exit on failure.
The source sends existing `cli::Input` values.
The daemon's shared ingress loop and production registry remain with #10 and
#20. The default simulation registry still uses the fake.

Before `SubmitChecks`, a maker/worktree caller supplies
`Observation::CheckPlan { node, sha, run: CheckRun { repository, base_sha, plan } }`.
`sha` is the maker's exact head. Send this before the maker's completion. `repository` is an absolute local Git
path. `base_sha` is the recorded full commit, not a moving branch. Each check
names its kind and exact pytest node IDs. The controller persists this binding
in `check_plans`, keyed by node and head. A late context for another head cannot
replace it. This uses the existing world-row store. It hydrates the delivery
with that context and the kernel's current head. An unbound plan stays unresolved
with an adapter error. No store migration or kernel rule changes are needed.

`Settings` names a private durable records directory, the trusted installed
`tl3_harness` package directory, the Python command, pinned image, honest CPU and
memory demand, progress allowance and result tool. Use a separate records
directory for each controller store. The Postgres controller claim permits one
adapter writer. The source writes durable `.done` completions after delivery. Retain them across restarts
and upgrades. The records directory also fences older write epochs; reads of
older jobs remain valid. Set `priority` to 0 for served work and -10 for agent
validation. The result tool is `python3` with the published `pbwait.py` argument.
The Python command must provide Python 3.14 on every eligible worker. For the
current fleet, `uv run --python 3.14 --no-project python` does that.

The adapter clones committed Git objects into its own clean checkout at the
exact head. Dirty maker files never enter it. A bundle retains both base and
head. The bundle, trusted harness copy, check plan and entry point form a clean
Git input that PB snapshots. Inputs remain unchanged while PB seals them.
`--seal-only` publishes no work. The adapter fsyncs the effect-to-action binding
before `--publish-sealed` can start work, then deletes the `.input` bundle.
Startup removes every leftover `.input`, including failed staging without a
binding. Reconciliation removes inputs for cancelled effects that never execute
again. Only `execute` seals or publishes. With no `.job`, reconciliation returns
`Absent`. A saved seal with no `.done` and no PB record also returns `Absent`.
The controller can then retry `execute` with the saved key after an input, look
or restart. A Cancelling effect instead settles through `cancel_absent`.
Withdrawal of an absent sealed action saves an empty completion and notifies
the source to forget its watch. There is no PB terminal event for that key.
Re-execution uses that saved CAS key,
even after the source checkout disappears or PB publishes a new runtime.
Existing queue records prevent another publication. Retries belong to the
kernel, and every submitted action has max attempts 1.

Queue and bell records decide when to read an ending. A queued or claimed job
stays pending. The adapter uses `pbwait.py --endings --until cleaned --wait-s 0`
to retrieve terminal evidence without waiting for execution. That observation
allowance never ends a job. A withdrawal decision alone cannot confirm cleanup.
Malformed queue records remain unknown. A digest mismatch, an unverified
receipt or an oversized result log reports `report: false, pass: false` once
and saves that completion. Result-tool spawn failures, unparseable output and
log I/O errors stay pending. Bell news or a restart retries them. Bell transport
backoff never retries a failed result tool.
Successful actions require a CAS receipt. Failed actions use PB's verified
immutable attempt logs and can have no success receipt.
A valid ending whose verified log has no valid harness report reports
`report: false`. Parsed reports bind the effect, base, head, check names, kinds,
selectors and current `in-process` custody. Only typed fields enter parsing.

The observation includes the action key, base, head and changed paths. The
controller interns those paths in the repository namespace and compares their
IDs with the node's footprint before handing `ChecksDone` to the kernel. Git's
NUL-delimited `base...head` diff starts at the merge base and includes both
names of a rename by disabling rename detection. Tests cover a divergent base,
a rename, a deletion, spaces, newlines and a case-only rename.
Evidence remains in the store event log and PB's verified result log.

The `prismabuild` controller test executable uses recorded done, failed and
withdrawn queue records from the client crate. Its tool fixture only writes
private queue files. It covers pending work, missing and malformed reports,
wrong heads, transport uncertainty, epoch fencing, withdrawal, SIGKILL followed
by resubmission, source delivery and footprint failures through the real
controller. Thread tests start the registered source, introduce new keys over
the adapter channel, deliver bell events, restart after delivery, and check
that idle sources make no bell requests or repeated ending reads. The channel
holds at most 64 keys; lag rebuilds the active set from durable records. Finished
jobs leave memory and get `.done` markers, which restarts skip. Withdrawal
skips jobs with `.done` before parsing. A later `SubmitChecks` for the same node
in a newer generation retires older `.job` and `.done` records only after
the newer ending has been delivered. The kernel
permits one in-flight effect per node, so the later submission proves the prior
effect has settled and its watch no longer reconciles. Current jobs and other nodes remain intact. Superseded seals with no ending
also retire, including never-published cancellations. Retirement fsyncs deletion of `.job` before deleting
`.done`; startup removes orphan markers after an interrupted retirement.
The last pair stays until that node delivers a newer ending. Teardown #13
should retire the final per-node records after all of that node's effects and
watches have settled. Neither age nor a timer permits deletion.

The source logs and skips bad `.job` records at startup or registration. A
directory failure, failed `.done` write or closed ingress receiver reaches
`RunningSource::join()` as an error. The source has
no timed wake-up and never scans the directory during ordinary bell waits.
It submits no fleet work from inside tests.

Generate the adapter review campaign from a clean detached worktree with
`python3 crates/controller/tools/prismabuild_review_manifest.py hosts > /tmp/tl3-pb-review.json`.
Submit that file with the published `pbcampaign.py`. It runs controller, client and store
process tests once on each fleet host. After those pass, generate and submit
`prismabuild_review_manifest.py workspace` for the full verification script.

The separate outer-session trial is reproducible from the repository root:

```sh
# Build this example through PrismaBuild first. Run its executable outside PB.
/path/to/pb_trial submit /absolute/empty/trial-directory "$IMAGE"
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbwait.py \
    --endings --until cleaned ACTION_KEY
/path/to/pb_trial observe /absolute/empty/trial-directory "$IMAGE"
```

The trial submits a fixture demonstrating check through the real adapter. Its
base fails and its exact head passes. The final command feeds the real ending
through the ingress parser and prints the checks observation. It refuses to run
inside an admitted action. Controller/store tests and this trial are separate.

## Add a worker role

For #11, #12 and #13, update each boundary together:

- Add the kernel role mapping in `adapters/workers/protocol.rs` `role`.
- Map and validate the reply in `map_reply`, using the package's role schema.
- Validate role inputs in `validate_inputs`.
- Build inputs and select the model ladder in `Supervisor::request`.
- Extend `lib.rs` `valid_role` and the document names in `translate`.
- Add process scenarios for accepted replies, invalid output, recovery and stop.

The forecaster supplies the size for `IngestDone`.
Until these boundaries support a role, its starts and stops use the worker fake.

Worker tests resolve `TL3_TEST_UV` first, then `uv` on PATH. Missing uv fails
with an installation message. They sync the locked Python 3.14 environment once
per test process. Run the process scenarios on dl380g10, sparky and sparklina
through PrismaBuild with `--tag HOST`. From a clean detached worktree, run
`python3 crates/controller/tools/worker_review_manifest.py workers` to generate
the three host actions, including controller and store process scenarios.
After they pass, generate the portable full check with
`python3 crates/controller/tools/worker_review_manifest.py workspace`.
Submit each generated manifest with `pbcampaign.py --detach`.

## Planning evidence

The supervisor runs `planner` with the accepted brief, `must_split`, accumulated
`findings`, protected paths and forecast settings. Planner retries select the
next configured planner model. The kernel owns the retry count and questions.
The worker reports leaf-rule violations in `violations`, alongside its forecast.
The controller maps those reports and `cant_meet` to `PlanCantMeet`. A valid split
maps to `PlanSplit`. A valid leaf interns its declared paths before `PlanLeaf`.

Ingest declares the root's files when it receives forecast settings. Worker code
uses the same pinned offline tokenizer as planning to measure the brief and those
files. The controller initializes size with `ceil(tokens / fit_tokens)`. The kernel
adds the credit reserve. An unrepresentable size fails explicitly rather than
wrapping or silently clamping the request's credit.

Supply a complete text snapshot in `repo.materials` for each existing path in
`repo.file_list`, through `Store::put_worker_materials`. Ingest and planner runs
materialize that snapshot into their own worktree. Paths must be canonical and
relative. New declared files have no base content. Deployment must preload the
pinned o200k_base data in `TIKTOKEN_CACHE_DIR`. Files above 1 MiB fail measurement.

`[planning.overhead_tokens]` contains named initial estimates. It is the seam for
later MLflow calibration. No MLflow calls run in this flow. Protected paths can
be extended in config, but the slice-one defaults cannot be removed.

Accepted plans live in `plans`; `plan_reports` retains each generation's evidence.
`findings` includes rejected-plan reasons and reports for the next planner run.
Callers that record failed checks or review findings append them to that same
text-list document before requesting a re-plan.

Every `Ask` notice atomically stores `question_context`, keyed by question ID,
and carries it as `Delivery.question_context`. `Store::question_context(id)` reads
it after a restart. `pieces` contains the proposed briefs, goal IDs and files.
Ticket #10's message pipeline consumes these briefs to draft the narrowing
question. This adapter does not draft messages.

Run `bash crates/controller/tools/verify-planning.sh` through PrismaBuild once
with each host tag, `dl380g10`, `sparky` and `sparklina`. It runs controller and
store process scenarios and the worker package's offline fake-model tests.

PrismaBuild bindings and completion markers live in per-node directories.
Ticket #13 teardown calls `prismabuild::Adapter::retire_node(node)` only after
the request closes and every effect and watch for that node has settled.
The call is idempotent and removes the final records as well as source watches.
`action_key(&Delivery)` locates a binding directly by node and effect key.
