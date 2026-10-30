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
without reading stdin. EOF ends a stdin-only simulation. Worker and webhook
sources keep the daemon running after stdin closes.

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

## GitHub ingress in simulation

`ingress::Source` runs on its own thread and sends `cli::Input` values to one
controller thread. Register a source with `Sources::register`. The stdin reader
uses that same interface. `Sources::run` drains persisted deliveries before
applying inputs and drives the controller until no work remains. Every source
exit is logged. An error or panic wakes and stops the controller loop immediately,
so `tl3d` exits and systemd can restart it while other sources are still alive.

To exercise the receiver against a local GitHub mock server, supply
`--github-listen 127.0.0.1:8081 --github-api http://127.0.0.1:8082/` alongside
`--simulate`. `github.webhook_listen` in the TOML file also sets the listen
address. With `--github-api` and no explicit listen address, the receiver defaults
to `127.0.0.1:8081`. Config and bind reject non-loopback addresses. `cloudflared`
on the same host forwards its tunnel to this socket. The mock App credentials come from `github.app_credentials_file`.
The API must be loopback HTTP in this simulation release. Ordinary stdin
simulation still needs no App credentials. Live startup and tunnel deployment
remain tickets #20 and #19.

POST deliveries to `/github/events`, matching the App manifest, with GitHub's
delivery, event and signature headers. Other methods return 405 with `Allow: POST`.
The status page and receiver share `src/http.rs`. Four handlers cap open
connections at eight, including queued sockets. Headers have an 8 KiB limit and
webhook bodies have a 25 MiB limit. Oversized bodies return 413, transfer
encoding returns 400, and webhook POST
requires `Content-Length`. Socket reads finish before reaching the receiver
thread, so a stalled client cannot hold delivery persistence or catch-up.
Accept errors are logged and skipped. Response writes have a total deadline too.

The library verifies the HMAC before parsing. The receiver stores
the event name, delivery id and unchanged raw bytes in `tl3_ingress` before returning
202. Replay parses them into the current `GitHubDelivery::Webhook` type, including
unknown event names. Rows that no longer parse log their delivery id and advance
every consumer past the row, so an older payload cannot crash-loop startup.
PR updates, check runs, check suites and merge groups remain available for their
owning adapters. Persistence errors log their `Display`,
reopen the ingress connection and retry once. Only a failed retry returns 503.
Delivery ids remain unique while retained. After retirement, repeated deliveries
still use the same durable intake and observation keys. A crash between applying
an input and advancing its consumer cursor replays the same intake or observation key.

`Webhooks` runs catch-up once at startup. Ticket #19 wires the tunnel supervisor
to `WebhookControl::gap` after an outage. Gap and stop notifications wake the
receiver's blocking channel. There is no polling timer. A failed persistence or
response marks a gap, which is retried at the next receiver event. Catch-up stores
`GitHubDelivery::CatchUp` with the full `RepoChanges` and its cursor in one
transaction, including PRs and check results. Comment URLs compare repository
names without case and use the trailing issue number. Rob's comments on issues
and PRs enter `github_comments` in both paths, deduplicated by repository,
installation and comment id. The steering controller classifies them.

Headers have a two-second total read deadline. Body reads get a separate total
budget of at least two seconds, scaled at 1 MiB/s by the declared length and
capped at 25 seconds for GitHub's 25 MiB limit. The listener is loopback-only
behind `cloudflared`. Failed reads never trigger catch-up because their bytes
are unauthenticated. Ticket #15 will read GitHub's failed-delivery records on a look.

One ingress connection inserts webhooks and catch-up rows in commit order.
Sequence cursors depend on this single-writer invariant: a concurrent inserter
could allocate a lower sequence and commit it after consumers had passed it.
The controller's separate connection advances cursors and retires rows.

### Register a delivery translator

PR #69 registers its Mergify translator, and #13 registers its PR translator,
on the controller before `Sources::run` or `ingress::drain`:

```rust
controller.register_github_translator("mergify", mergify_translator);
```

Implement `ingress::GitHubTranslator::translate(&mut self, &GitHubDelivery,
&tl3_store::Snapshot) -> Result<Vec<cli::Input>>`. Translate both `Webhook` and
`CatchUp` variants into observations. Validate the served repository and
installation before producing evidence, and use stable observation keys across
webhook and catch-up overlap. Translators never call the kernel.

`ingress::drain` reads the stored deliveries in arrival order, calls the
translator on the controller thread, applies its inputs and then persists its
`github-translator:<name>` sequence in `tl3_ingress_consumers`. Each translator has
an independent cursor. A new name starts at intake's current durable position,
skipping consumed history even if another translator still retains it. Pending
deliveries and catch-up supply recent state. Reusing the name after restart resumes it.
An error leaves that delivery's cursor unchanged. Inputs must be idempotent
because a crash after applying them can replay the delivery. Intake and cursor advances delete rows in the same transaction once intake and
every registered translator have passed them. No clock drives retirement. The
registered set is reconciled before each drain, so removed translators stop retaining
rows. Each translator loads at most one world snapshot per batch.

`adapters::github::Adapter` uses the REST client for kernel notices. It invokes
`workers::writer::Writer`, checks every draft and allows two correction attempts.
Three rejected drafts return a permanent acknowledgement, so recovery does not
restart the writer loop. A worker transport error stays uncertain.
`FakeWriter` returns fixed prose in simulation. The seam uses the existing writer
role's protocol v1 input and reply shapes. Questions retain the kernel's allowed
options. Closing summaries close the issue, except when the request is replaced.
PR effects still use this module's fake until ticket #13. Incident notices stay
on the fake until the operations issue is wired in. Replacement and correction
notices create linked issues with the `tl3` label.
The checker uses the request's own `Delivery.paths` and any further paths
supplied in the adapter policy. No build script inventories TL3's checkout.
It rejects both `:line` and `:line:column` suffixes.

The `github` integration target uses the recorded webhook fixtures, the actual
HTTP receiver, a local REST server and embedded Postgres. It covers delivery
persistence, unauthorized labels, gap recovery, duplicate reconciliation, writer
rejections, epoch fencing and SIGKILL after posting before acknowledgement.

`adapters::simulation_with_workers` replaces ingest, audit and planning with Python worker
processes and returns a `WorkerOutputSource`. Making and review remain on the worker fake. Supply issue and repository text through `Store::put_worker_materials`
before dispatch. Audit reads the stored ingest reply. An ingest revision reads
the audit's objections. Missing materials block dispatch.

`tl3d --config FILE --simulate DIRECTORY --workers` registers the worker output
source alongside stdin and optional webhooks. It uses configured Python for
ingest, audit and planning, with the remaining roles and upstreams simulated. Add
`--github-listen 127.0.0.1:8081 --github-api http://127.0.0.1:PORT` to run both
sources against a local GitHub mock. Without `--workers`, workers stay simulated.
Worker replies enter the same controller channel as webhook notifications. A
worker source error stops the daemon even while webhooks or stdin remain alive.
The source writes progress to a separate store table. Simulations can still call
`poll_once` and feed its channel to `Controller::observe`.

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

For PR #64's receiver and status review evidence, create a clean detached worktree
at the final commit. From that worktree, submit the three host checks:

```bash
python3 crates/controller/tools/github_review_manifest.py > /tmp/tl3-github-review.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-github-review.json
```

The manifest pins the controller, GitHub and Postgres test targets to `dl380g10`, `sparky`
and `sparklina`. Controller targets include the status page and process tests.
After they pass, submit the full workspace `tools/verify.sh` command shown above
once with `--anywhere`. Record every action key and result in the PR body.

## Add a worker role

For each role, update these boundaries together:

- Add the kernel role mapping in `adapters/workers/protocol.rs` `role`.
- Map and validate the reply in `map_reply`, using the package's role schema.
- Validate role inputs in `validate_inputs`.
- Build inputs and select the model ladder in `Supervisor::request`.
- Extend `lib.rs` `valid_role` and the document names in `translate`.
- Add process scenarios for accepted replies, invalid output, recovery and stop.

The classifier uses `Role::Classify` and the same supervisor, schema validation,
model configuration and output source. It has a separate comment job because
it can run while request work continues or after the request closes. Its reply
becomes `Observation::Classified`, rather than a reply to the request's current run.

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

## Steering by comment

`steering.rs` queues each stored comment from the configured actor once. Startup
queues comments stored before steering was installed too. A durable `comment_jobs`
row owns one classification at a time. The job stores the phase and open question
shown to the classifier. A reply keeps that question's ID even if the question
changes while the worker runs. Completed classifications leave the watch set and
the pending queue. Their stored comments retain the result for deduplication.
No timer scans comment history.

Pause, resume, stop, change and named answers enter the kernel as commands.
The controller assigns their sequence when applying the comment, so webhook
arrival order does not depend on GitHub's numeric comment IDs. The job and its
command commit together. Remarks emit no command. An unsure reply to an open
question becomes `Answer::Unclear`; the kernel allows one clarification. Without
an open question, `Command::ClarifyComment` emits one short question without
interrupting work. Both question paths use the GitHub writer and checker.

The kernel emits `FileReplacement` after closing as Replaced, or `FileCorrection`
for a complaint on a closed request. The GitHub adapter creates an issue with a
stable effect marker, the `tl3` label and a link to Rob's authorizing comment.
A correction also records his request for a revert. Confirming or reconciling
that issue starts a new kernel request with the original request's authorization.
The original request stays closed. These changes use existing store tables and
do not add a SQL migration.

The GitHub integration target drives `tl3d`, signed recorded webhook fixtures,
the Python worker with a fake model, and a local REST server. It covers command
deduplication, actor authorization, remarks, questions, stale answers, classifier
reattachment, replacements, corrections and a queued PR merging while paused.
In simulation, a `hold-merges` file in the fake records directory lets the
Mergify fake retain a queued PR until the test supplies merge evidence.

Generate ticket 14's three host checks from a clean commit:

```bash
python3 crates/controller/tools/steering_review_manifest.py hosts > /tmp/tl3-steering-hosts.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-steering-hosts.json
```

The `workspace` and `proofs` selections generate the portable full check and the
three affected command proofs. Inspect each terminal result, logs and CAS receipt.

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
