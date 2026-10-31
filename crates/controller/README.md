# TL3 controller

`tl3-controller` connects the pure kernel to the Postgres store and upstream
executors. Read [the architecture note](../../docs/architecture.md) for the
transaction and recovery contracts.

All commands use the same TOML configuration as `tl3-config`. The daemon runs
production adapters by default, or simulation with `--simulate`. Status commands read the configured store
and do not load GitHub or model credentials.

The daemon uses `Config::load`, which checks all secret-file metadata. Status
uses `Config::load_status`, which checks only the optional store password.
Only the store reads its password at runtime.
`tl3 status` uses `Reader::open`. It never migrates or writes and
reports an uninitialized database clearly.

```text
tl3 status --config /absolute/path/to/config.toml
tl3 status-serve --config /absolute/path/to/config.toml
tl3d --config /absolute/path/to/config.toml
tl3d --config /absolute/path/to/config.toml --simulate /absolute/path/to/fake-records
```

In simulation, the daemon reads one JSON `cli::Input` per line on stdin. For example:

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
registers `worktrees::Real` for all four worktree effects against local bare
repositories over `file://`. The other four upstreams stay fake. The worker
fixture commits a real change and reports its head. Separate runs move main
with a nonconflicting or conflicting commit. The conflict reply passes through
the adapter and controller inbox to the kernel, which returns to Making and
spends one credit in the same attempt and round.

Each adapter ticket extends `Delivery` with the fields its real client needs.
The tracer does not predefine issue text, branch text, or PR text fields.

`tl3 status` and the page share `status::load`, one typed model and one set
of columns. They show the issue title and number, phase, wait, credit, last
progress and open incidents. Older requests without a title show "title not
recorded". Both progress and incidents show "none recorded" until their store
data exists. `status::load_request_details` reads active worker progress or the
retired run summary. Ticket #15 adds incident details. It takes all
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

`adapters::simulation_with_workers` replaces ingest, audit, planning, maker and review
with Python worker processes and returns a `WorkerOutputSource`. Review needs
committed Git and check evidence. Supply issue and repository text through `Store::put_worker_materials`
before dispatch. Audit reads the stored ingest reply. An ingest revision reads
the audit's objections. Missing materials block dispatch.

Maker materials use a `maker` object in that same per-node record. It contains
the node's `brief`, `goals` with stable string IDs, `plan.checks`, and any
accumulated `feedback`. A fix round requires feedback. The controller records
failed checks, review requests and can't-meet reports in `maker_findings` in the
same transaction as the kernel reply. The supervisor adds these to supplied
findings on every subsequent attempt. Ingress supplies the brief slice and
detailed findings; the supervisor supplies authoritative `plan.files`,
`attempt`, `round`, context and fit limits, PrismaBuild settings, and Hindsight
endpoint and bank from the delivery and config. Other roles can also use named objects
in the materials record; the flat ingest/audit format still works.

The maker uses `worktrees_root/HEX_OWNER/HEX_REPO/issue-ISSUE-node-NODE`, matching
#61's worktree adapter. Owner and repository are lowercase UTF-8 encoded as hex.
It never uses an empty per-run directory as a checkout.
The acceptance fixture uses the merged real worktrees adapter.
Attempt N selects `models.maker[N - 1]` for both rounds. The kernel alone
advances attempts. Each attempt has a conversation outside the worktree under
`workers.runs/sessions/NODE/plan-REPLANNED/attempt-N.json`. Round 1 resumes
it. A fitting `needs_files` resumes at round 0 within the same attempt and plan.
The resume decision checks the stored reply against the delivery attempt and
re-plan flag, so a restart at attempt 1 after a re-plan starts fresh.
A checkpoint records the starting commit for each
Making visit, including rebase-needs-changes revisits in the same attempt and round.
The controller records the generation of the last accepted maker reply. Until it
accepts a reply, retries retain the checkpoint across new generations and count
commits left by a crash or a failed session save. An accepted reply ends the visit.
Each attempt stores one checkpoint with its generation and head; visits replace it
so retries cannot accumulate checkpoint files.
An accepted `maker_session` without a numeric generation is rejected before a
worker starts, with `maker session has no accepted generation`. Slice one is
unreleased; legacy sessions need explicit repair rather than guessing whether
an old checkpoint belongs to the current visit. An absent session still starts
normally and can recover unaccepted commits.
Only clean, resumable completions save conversation history. Out-of-room runs
preserve any previous conversation without saving the exhausted one.

After durable work drains, the supervisor deletes a closed node's session
directory only when no inbox entry, pending effect, retained watch or unretired worker can
reconcile that node. It queries only remaining session directories after retiring
runs, without loading historical world rows. Deletion errors are logged and retained
for a later pass; they do not fail the controller's idle drain.
Open requests keep their sessions. The directory itself survives a crash as
the cleanup queue, and the next drain repeats deletion. No clock triggers cleanup.
Worker code commits a completed change and computes a conservative fit bound
for additional files. The controller interns returned heads and requested paths
before delivering kernel replies. It stores the latest reply as `maker_replies`.

Maker acceptance runs the actual Python entry point and maker graph with a
scripted model, fake read-only Hindsight, and recorded self-check results.
The combined acceptance also uses #61's real worktrees adapter for checkout,
publication and cleanup. Focused worker cases keep their local Git fixtures.
The checks use the merged real PrismaBuild adapter, its result source, and recorded
PrismaBuild queue and ending records. No nested submission runs in acceptance.
Scenarios cover success, failed checks through both rounds of all three attempts
and one re-plan, an actual commit outside the footprint, requested files,
context exhaustion, unmet goals, invalid output, recovery, and stop.
Feedback items have `kind`, `report` and `goal_ids`. Actionable evidence adds
`failed_tests` with `test` and `message`, `outside_paths`, and `ci_checks` with
`name`, `summary` and optional `url`. The report describes the failure in plain
language. `Observation::CiFailed` persists named CI evidence in the same
transaction as its kernel fact. A bare non-infrastructure CI fact still records
feedback, explicitly stating when the upstream supplied no check details.
Infrastructure failures and stale replies do not add maker findings.
Reviewer blockers supply their problem, evidence and cited goal IDs to the next
maker run, with up to 20 blockers per review. Each problem and evidence is
capped at 1,000 characters, with a 16,000-character total per review. Accumulated maker reports enter the planner's
`findings` only when the request re-plans.

Production registration supplies committed repository materials. Before maker
dispatch, the supervisor translates the accepted brief and planner checks into
the stored `maker` object, with stable goal IDs. It also stores the initial
check plan. The supervisor adds kernel fields and deployment settings.
Mergify's current queue CI fact supplies no check text. Maker feedback records
that details are unavailable when it receives a bare fact.

The merged PrismaBuild checks observation supplies changed paths and up to 20
failing test IDs with messages capped at 1,000 characters each. The trusted
harness retains pytest assertion messages; older reports without messages still
parse and receive an explicit fallback. The controller records paths outside the
node's footprint and failed tests in `maker_findings`. Missing goal materials
retain unbound findings with an empty goal list and a log entry. Invalid goal IDs
return an error. CI feedback keeps up to 20 checks, with 1,000-character summaries,
4,096-character links and a 16,000-character finding report.

`tests/maker/records` retains the actual maker-to-harness trial's terminal record,
CAS receipt and payload. Acceptance verifies the hash and compares its code, tests
and harness configuration with a new maker change. It then runs the real checks
adapter against recorded queue evidence. The separate trial checks the committed
result independently through PrismaBuild's managed container.

Generate all-host verification with
`python3 crates/controller/tools/worker_review_manifest.py maker`, then submit
the manifest with `pbcampaign.py --detach`. The three pinned actions run maker,
worker, supervisor, PrismaBuild adapter, GitHub ingress and worktrees scenarios.
They also cover process exit during both opening and reading `/proc/<pid>/stat`.
The open case extends main's ESRCH fix; permission errors still propagate.
The daemon fixture makes and commits a real change through the shared ingress loop.
A separate portable action runs the Python worker and harness suites and
`tools/verify.sh`. These tests use fixtures and never submit nested jobs.
Add `--trial-image <immutable-image>` to the manifest command to include a new
real maker-to-harness trial in its own admitted action. The image must already
exist on an eligible worker. `tools/maker_trial.py` requires admission and
never submits work. The submitting session runs `pbcampaign.py` outside every
admitted action.

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
The mock API must be loopback HTTP. Ordinary stdin simulation needs no App
credentials. Without `--simulate`, the daemon uses the configured App and real
adapters. Follow [GO-LIVE.md](../../deploy/dl380g10/GO-LIVE.md) for deployment.

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

Register the Mergify translator on the controller before `Sources::run` or
`ingress::drain`. It handles PR updates as well as queue facts:

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
Three rejected comment clarifications post a fixed, checked request to rephrase.
Other rejected notices fail permanently and post nothing. Writer transport errors
leave the delivery unconfirmed.
The reply uses the original effect marker, so recovery does not restart the
writer loop once the reply exists.
`FakeWriter` returns fixed prose in simulation. The seam uses the existing writer
role's protocol v1 input and reply shapes. Questions retain the kernel's allowed
options. Closing summaries close the issue, except when the request is replaced.
PR effects use the same adapter's publishing implementation. Incident notices use
the operations issue. Replacements start a new request on the same issue from
Rob's text. Corrections create linked issues with the `tl3` label.
The checker uses the request's own `Delivery.paths` and any further paths
supplied in the adapter policy. No build script inventories TL3's checkout.
It rejects both `:line` and `:line:column` suffixes.

The `github` integration target uses the recorded webhook fixtures, the actual
HTTP receiver, a local REST server and embedded Postgres. It covers delivery
persistence, unauthorized labels, gap recovery, duplicate reconciliation, writer
rejections, epoch fencing and SIGKILL after posting before acknowledgement.

`adapters::simulation_with_workers` replaces ingest, audit, planning, maker and review
with Python worker processes and returns a `WorkerOutputSource`. Supply issue and
repository text through `Store::put_worker_materials` before dispatch. Review also
needs committed Git and check evidence.

`tl3d --config FILE --simulate DIRECTORY --workers` registers the worker output
source alongside stdin and optional webhooks. It uses configured Python for
ingest, audit, planning, maker and review, with other upstreams simulated. Add
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
The deployed unit uses `KillMode=control-group`: a service restart stops workers,
supervisors and other local children. Recovery reconciles durable records before
retrying. A process-only controller crash can still reattach to a live supervisor.
See [the startup procedure](../../deploy/dl380g10/GO-LIVE.md) for restart checks.

Sync the worker environment once at deployment with
`uv sync --locked --project /srv/tl3/workers --python 3.14+gil`.
Set `workers.python` to its `.venv/bin/python`. Each launch uses that interpreter
directly, without uv cache or venv locks. `workers.command` contains Python's
arguments, normally `-m tl3_workers run`.

The source watches only active runs with inotify for output and exit records,
and pidfds for process exits. A durable per-key notification registers a new
attempt through the `.ready/` inbox. Completed runs leave the watched set.
Startup reads only the store's worker watches and their run attempts. It never
opens historical run directories or replies. Bad historical records are logged and skipped during startup. Malformed live
notifications and I/O errors fail the source loudly so the daemon restarts.
Only vanished-file races are skipped in live refreshes.
Stop indexes active runs by node. Attempt events matter only for `claim.json`,
`process.json`, `stdout.jsonl`, and `exit.json`. An inotify queue overflow causes
one rescan of watches and the notification inbox, then event-driven ingress resumes.
Startup consumes stale inbox markers and removes legacy `.ready-*` markers for
watched keys. Retirement removes them for finished keys. Neither scans history.

After the controller drains work, `Executor::retire` reclaims runs whose effects
are terminal in the store and have no watch. A Confirmed start remains watched
until its operation ends, so an acknowledgement alone never permits deletion.
Cleanup fences delayed launches and proves both processes dead, removes the run
directory and ready marker, syncs the parent directories, then deletes the
`tl3_worker_progress` row. That row is the cleanup queue. A crash at any step
leaves it available for the next controller drain, including after restart.
Missing files are harmless. Failed cleanup logs the key and retains its row.
Corrupt or foreign process identities are marked as blocked, logged once, and
skipped by later drains and restarts until operator repair.
Retirement uses no clock. Claims are stored before creating run directories,
so a crash before launch also leaves a cleanup row.
Deleting a progress row folds its last progress into one summary per open request.
Closing a request removes its summary. Status uses it when no active run exists.

Only a valid role reply followed by exit zero is accepted. A recorded failed
exit reports `RunFailed`. A lost supervisor record is absent only after both
identities are proven dead and delayed launches are fenced. The controller
reports absence of an acknowledged worker as `RunFailed`; an uncertain start
can retry the same key. The source emits only `Found` evidence, never `Absent`
or a cancelled claim. The controller watch owns confirmed absence. Ticket #15's
looks must wake the controller when no other input arrives; a source pidfd wake
alone does not send an ingress observation for absence.
If a supervisor dies while its worker lives, recovery waits for that worker.
A valid reply without a recorded exit is discarded, and the run is redone.
This is the safe path because nobody proved a successful process exit.
No clock or liveness threshold makes this decision.

`Store::worker_progress` reads the last progress for every run, optionally
filtered by request node, until retirement. `Store::worker_document` reads briefs,
goals, audit replies, plans, and findings. Replaying output does not refresh
progress time. These reads do not
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

The real Mergify adapter replaces its registry entry with one call:

```rust,ignore
registry.register(Upstream::Mergify, mergify::Adapter::new(clients, epoch, records)?);
```

Keep `records` in a persistent directory. It holds the accepted epoch, queue
check baselines and receipts by effect key. A label write alone leaves enqueue
uncertain. Mergify must report queue membership or the attempt's terminal result.
An outside commit between Mergify's head check and adding `tl3-queue` can leave
a stale label; the new head has no approval check, so it cannot merge.
The kernel has no dequeue effect, so this adapter adds none. The GitHub adapter
owns approval checks and opening and closing PRs.

Register queue translation on the controller before draining ingress:

```rust,ignore
controller.register_github_translator("mergify", mergify::Translator::new(clients)?);
```

The hook translates persisted webhook and catch-up deliveries on the controller
thread. The receiver stores verified events before returning 202. Queue reads
never run while acknowledging a webhook. A failed translator keeps its cursor
for replay. GitHub does not automatically redeliver failed webhooks.
Merge-group notifications request authoritative reads of open nodes.

Queue retries persist intent, remove a remaining queue label, then add it again.
A successful label write also persists an applied marker. A later person removal
confirms the attempted enqueue and reports dequeue evidence instead of restoring
the label. In-progress queue checks can confirm an enqueue.
Once an effect is terminal and its watch is removed, durable retirement deletes
its intent, applied marker and receipt. Restart retries interrupted cleanup.

The controller commits each queue fact's identity with its kernel step in
`queue_facts`. Person actions use GitHub issue-event IDs in both delivery paths.
These rows are pruned when a node closes. Catch-up and merge-group reads exclude
closed nodes, except that merge webhooks, catch-up and queue-check events can
correct a closed node without a recorded merge. Merges report the PR's actual head. A successful Mergify check
reporting "Merged via merge queue" proves queue CI passed before the merge.
Outside merges use checks on the actual merge commit. Completed `neutral` and
`skipped` checks pass. That commit usually has no CI yet when the merge webhook
arrives, so an outside merge usually reports false and the kernel raises an
unapproved-merge incident. This reflects the merge commit's own evidence.
A final Mergify check with any other title, such as "Merge queue is ready",
is no queue evidence. It uses the same merge-commit CI fallback as an outside merge.
If the merged head equals the node's current approved head and the Mergify queue
check is not final, no merge fact is emitted yet. Its completed check-run event
or catch-up supplies the final evidence. A merge of any other head is reported
immediately, even while the queue check runs, so the kernel raises its incident.
Only the queue check from the Mergify app triggers check-event reads; check suites and other check runs are ignored.

A reconciliation completion includes `executor::ProgressToken::Mergify` even
when it has no observations. The public `mergify::Progress` contains GitHub's
`merged_at` and `merge_commit_sha`, plus the selected queue check's `id`, `status`,
`conclusion` and `title`. A missing check is `None`. The controller stores
`executor::WatchProgress` in the watch row's `progress` field, with `previous`
and `current` tokens. `changed()` compares them. The first read has no previous
token. Repeated identical reads leave equal tokens and cause no further commits.
Progress bookkeeping does not schedule another look. The driver still visits
other watches in the same pass. The comparison survives restart and leaves with
the watch, so it does not retain finished history. Retirement errors are logged
and retried on a later drive. They do not stop inbox or effect processing.
Mergify's `retire_delivery` resolves an adapter only for delivery files requiring
cleanup. Worker retirement keeps its separate store-based `retire` hook; errors
from that hook are also logged without stopping the driver.

Allowance looks compare the durable watch token. If it remains unchanged past
Mergify's no-progress allowance, the controller sends `Fact::UpstreamStalled`.
The kernel raises an incident in Queued. Its text includes GitHub's merge time
and Mergify's queue-check title and status from that watch.

Create a clean detached worktree at the final commit, then submit its host checks:

```bash
python3 crates/controller/tools/mergify_review_manifest.py > /tmp/tl3-mergify-review.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-mergify-review.json
```

The manifest runs controller, GitHub, Mergify and store tests on `dl380g10`,
`sparky` and `sparklina`, including the crash simulation smoke run. After they pass,
generate `mergify_review_manifest.py workspace` and submit it to run
`bash crates/controller/tools/verify.sh` once with `--anywhere`. The scenarios use local HTTP servers and embedded Postgres.
See [the fixture notes](tests/fixtures/mergify/README.md) for recorded and synthetic
inputs. Real queue payload confirmation remains the sandbox trial on #17.

The real checks adapter lives in `adapters::prismabuild::Adapter`. Register it at
`Upstream::PrismaBuild`. Its `source().spawn(sender)` starts the ingress thread;
keep the returned handle alive. Its `join()` returns source errors or panics to
the daemon supervisor, which must stop the ingress loop and exit on failure.
The source sends existing `cli::Input` values.
It also implements `ingress::Source`, so the shared ingress supervisor registers
it with `sources.register(adapter.source())` and propagates its errors.
Production registers this adapter and its bell source from config.
The default simulation registry still uses the fake.

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

The PrismaBuild tool must be `pbrun` itself, or a wrapper that `exec`s it.
The parent-death signal protects only the process the adapter execs. A wrapper
that starts `pbrun` without `exec` leaves it as a grandchild that can survive a
controller crash.

The adapter clones committed Git objects into its own clean checkout at the
exact head. Dirty maker files never enter it. A bundle retains both base and
head. The bundle, trusted harness copy, check plan and entry point form a clean
Git input that PB snapshots. Inputs remain unchanged while PB seals them.
`--seal-only` publishes no work. The adapter fsyncs the effect-to-action binding
before `--publish-sealed` can start work, then deletes the `.input` bundle.
Startup removes every leftover `.input`, including failed staging without a
binding. Reconciliation removes inputs for cancelled effects that never execute
again. Only `execute` seals or publishes. With no `.job`, reconciliation returns
`Absent`. With a saved seal and no queue row, the adapter consults the cleaned
ending first. A readable ending supplies the result. A waiting ending permits
`Absent`; tool errors stay unknown. The controller can then retry `execute` with the saved key after an input, look
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
holds at most 64 node IDs; lag rebuilds the active set from durable records. Finished
jobs leave memory and get `.done` markers, which restarts skip. Withdrawal
skips jobs with `.done` before parsing. A later `SubmitChecks` for the same node
in a newer generation retires older `.job` and `.done` records only after
the newer ending has been delivered. The kernel
permits one in-flight effect per node, so the later submission proves the prior
effect has settled and its watch no longer reconciles. Current jobs and other nodes remain intact. Superseded seals with no ending
also retire, including never-published cancellations. Retirement fsyncs deletion of `.job` before deleting
`.done`; startup removes orphan markers after an interrupted retirement.
Records live under `<records>/<node id>/`, keyed by encoded effect key.
Supersession and withdrawal list only that node's files.
`action_key(&Delivery)` locates the binding directly. Startup moves flat bindings
into node directories while preserving their completion markers.
The last pair stays until that node delivers a newer ending. After the request
closes and every effect and watch for that node has settled, the adapter retirement hook
calls `prismabuild::Adapter::retire_node(node)`. The call is idempotent and removes
the final records and source watches. Neither age nor a timer permits deletion.

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

The systematic [crash simulation](../../docs/crash-simulation.md) uses real
SIGKILL cuts inside transactions and around dispatch and reconciliation. Its
default `merge` smoke scenario covers all thirteen save points. Set
`TL3_SIM_FULL=1` for the full matrix and effect-and-phase coverage assertion.
Its seed and optional scenario and save-point filters appear in failure output.
Generate the required three-host and two-portable campaign with
`python3 crates/controller/tools/crash_manifest.py`. All execution still goes
through PrismaBuild.

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


## Worktree adapter

`worktrees::Real::new(&config.worktrees_root, epoch)` uses the existing configurable
root. Add each repository with `serve_github(identity, local_repository, github)`.
The GitHub client must match the repository's installation. The client supplies
cached installation tokens at runtime. For simulation, use `serve_local` with a
local bare remote. Local operations never obtain or pass a token.

Register the adapter once with `registry.register(Upstream::Worktrees, real)`.
The daemon still requires explicit simulation; production ingress and startup
registration belong to their adapter tickets.

The adapter owns branches named `tl3/issue-N-node-M`. Checkout paths beneath the
configured root separate repositories and requests. `Real::branch` and
`Real::checkout` expose those names for worker registration. One controller
process and its Git children own each repository. Stop the whole service process
group before starting a replacement. Do not run another Git writer or maintenance
process against an owned repository.

Keep `<common-git-dir>/tl3-controller` across restarts. It binds the repository
and worktree root, advances the epoch fence as soon as the repository opens, and journals immutable
operation intents and terminal evidence. Changing that root requires an explicit
migration of existing checkouts and records. Records use atomic replacement and
sync both the file and its directory. No store migration is required.

Prepare pins fetched main before creating missing resources. Git's administrative
lock `<common-git-dir>/worktrees/<id>/locked`, with the exact reason `initializing\n`,
is the evidence of interrupted checkout population. Any key may repair it only
while the branch head equals the durable base ref. Repair runs
`reset --hard <branch>` and unlocks after verifying the files, even if main has advanced.
Without that lock, Prepare only verifies registration and the request branch.
It preserves maker commits, edits and untracked files across fix rounds, new
attempts and stale intent reconciliation. The next maker sees them in `git status`.
Earlier intents remain readable. Their checkout ownership fields are ignored.
Push pins the delivery's exact head and leases against the last head this adapter
published. Rebase pins its target before execution, so recovery cannot silently
use a newer main. A clean rebase reports `Observation::Rebased`; the controller
interns its SHA before the kernel receives `RebaseClean`. A conflict reports
`RebaseNeedsChanges` after aborting and verifying a clean checkout at the old
head. The kernel alone selects the next phase.

Teardown records that the checkout is clean before deletion begins. It also accepts
Git's initializing lock plus head equal to the durable base, even when an
interrupted checkout left tracked files. It reads this evidence directly without
scanning the journal. Removal keeps the lock until Git deletes the checkout.
A retry may finish removal if its only changes are deletions. Other edits and new
files are refused.
It leases deletion of the recorded remote head, then removes the local branch
and base. A dirty checkout or changed remote
ref returns an adapter error. Unknown outcomes keep their original effect key.
Interrupted current-epoch operations reconcile against Git and finish their
original intent. Older epochs may read terminal evidence, but cannot resume
unfinished Git writes. Once fenced, an unfinished intent with no terminal Git
evidence returns `Absent`, so a Cancelling delivery can settle. Git steps converge;
transport errors remain unknown. No duration or silence establishes absence.

The `worktrees` test executable kills adapter children with SIGKILL before and
after Git applies each effect, including conflict and stale-epoch cases. The
workspace verification script runs it, the mixed registry lifecycle, and the
existing controller and store scenarios through PrismaBuild.

Tickets #12 and #13 open `tl3_worktrees::Worktrees` for the served repository and
call `branch_state(&Real::branch(delivery)?)` to read its recorded base and current
head. The tuple is `(head, base)`. `changed_files(&base, &head)` returns the changed
paths for the check harness and publication footprint. After a clean rebase,
read both values again: the base and head move together. `Delivery::head_sha`
remains the exact commit requested by an individual push effect.


`tools/worktree_review_manifest.py worktrees` generates the three host-tagged
PrismaBuild checks from a clean detached worktree at the final commit. Each runs
all controller and worktrees targets, including the 50 SIGKILL boundaries and real
killed-add recoveries. Submit its JSON with `pbcampaign.py --detach`. After those
pass, submit `tools/worktree_review_manifest.py workspace` the same way to run
`verify.sh` once with portable placement. Record every action key and result in
the PR body. See the [worktrees recovery guide](../worktrees/README.md#recovery)
for manual initialization locks and half-registered adds that need operator cleanup.


## Add a worker role

For each new worker role, update these boundaries together:

- Add the kernel role mapping in `adapters/workers/protocol.rs` `role`.
- Map and validate the reply in `map_reply`, using the package's role schema.
- Validate role inputs in `validate_inputs`.
- Build inputs and select the model ladder in `Supervisor::request`.
- Extend `lib.rs` `valid_role` and the document names in `translate`.
- Add process scenarios for accepted replies, invalid output, recovery and stop.

The classifier uses the supervisor's `classifier` mapping and the same schema validation,
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
row owns one classification per request at a time. The job stores the phase and
open question shown to the classifier. A reply keeps that question's ID even if the question
changes while the worker runs. Completed classifications leave the watch set and
the pending queue. Stored comments retain the result while the request is open.
After closure and delivery of any reply or correction, the controller deletes the
comment row. Its retirement event keeps the comment ID for indexed deduplication.
No timer scans comment history.

Pause, resume, stop, change and named answers enter the kernel as its existing
commands. The controller assigns their sequence when applying the comment.
The classification, job retirement and command commit together. Remarks and
stale answers emit no command. `Output::Comment` carries ingress work directly
to adapters, without a kernel effect or notice.

An unsure comment gets one short question through the writer's `clarification`
message kind, checker and marker reconciliation. A failed classification retries
twice with distinct attempt keys. After that, a checked question quotes a short
excerpt and asks Rob to rephrase. Rejected writer drafts receive their violations
for up to two reruns. Exhaustion posts a fixed, checked request to rephrase.
`comment_questions` tracks the conversation by the original comment key. The next comment is classified with that conversation as
context. Completed conversations leave the pending set. An explicit unclear
answer to a kernel question becomes its existing `Answer::Unclear`; the kernel
allows one clarification. Comment uncertainty never changes a kernel question.

The existing `FileReplacement` notice starts a replacement on the same issue
after closing as Replaced. The closure commit records Rob's new text in a
`replacement_pending` row keyed by the old request. Intake or the next drive
starts the new request from that row and deletes both replacement rows in the
same commit. A late merge still corrects the old request and does not cancel
the pending replacement. Startup needs no GitHub read. The issue stays open,
and intake, status and comment routing use its latest request. A complaint on a
closed request files a correction directly from the comment pipeline, keyed by comment ID. The closed node receives no event.
Only corrections create a new issue. The GitHub adapter uses a stable marker, the
`tl3` label and a link to Rob's authorizing comment. A correction records his request for a revert.
Marker reconciliation recovers an issue created before an acknowledgement.
Its issue query is bounded by the authorizing comment's timestamp and sorts by
creation time. The same timestamp remains attached through retries and recovery.
Both paths use the same authorized intake as a labelled request. The new ingest
reads the replacement text or the correction issue's text and inherits the original repository snapshot
through the durable request link. Links point to earlier requests, so chains
terminate at the original materials. No SQL migration is needed. Store versions
1 through 5 and `crates/kernel` are unchanged from main.

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

The `steering` selection runs the GitHub target, including steering races,
on all three hosts. The `writer` selection runs the Python writer tests once.
The `workspace` selection generates the portable full check. Inspect each
terminal result, logs and CAS receipt. No kernel rule or proof changes.

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

Supply the text snapshot in `repo.materials` for each UTF-8 text path in
`repo.file_list`, through `Store::put_worker_materials`. Listed paths without text
receive a binary marker in the worktree. They do not block dispatch, but declaring
one for measurement fails instead of treating it as an empty new file. Ingest and planner runs
materialize that snapshot into their own worktree. Paths must be canonical and
relative. New declared files have no base content. Deployment must preload the
pinned o200k_base data in `TIKTOKEN_CACHE_DIR`. Files above 1 MiB fail measurement.

`[planning.overhead_tokens]` contains named initial estimates. It is the seam for
later MLflow calibration. Their sum must be below `limits.fit_tokens`.
No MLflow calls run in this flow. Protected paths can
be extended in config, but the slice-one defaults cannot be removed.

`plans` holds the latest schema-valid planner reply, accepted or rejected.
Malformed output fails the run and does not replace it. The controller does not
write unused `plan_reports` or `reviews` documents. Review blockers are retained
in `maker_findings`; accepted review visits are retained in `reviewer_session`.
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
Generate all three host actions with
`python3 crates/controller/tools/worker_review_manifest.py planning` and submit
the manifest through `pbcampaign.py --detach`.

The planner process scenarios cover malformed output, a controller killed while a
planner is running, and stopping a live planner. Malformed replies fail without
storing a plan. Recovery reattaches to the same process and key with one attempt.
Stop proves the process has ended and stores no plan. The leaf, split, rejected
leaf, invalid split and cannot-meet scenarios also check persistence, narrowing
context and retry findings.

## Hindsight write-back

Store migration 6 remains `0006_worker_wait.sql`, as deployed by the MVP.
Migration 7, `0007_hindsight.sql`, adds the memory outbox. Migration 8,
`0008_memory_parking.sql`, adds parking and key-rejection health. Migration 9,
`0009_memory_recovery.sql`, timestamps parking for retention. Upgrading
a schema-6 database preserves its recorded SQL and existing worker waits.

Every worker role uses the same runtime summary collector. Protocol v1 requests
can carry `memory` with a controller-owned `summary_file`, repository, issue,
request and epoch. The collector atomically replaces and fsyncs `memory.json`
at file and test tool boundaries and when the run ends. It records paths, test
selectors, outcomes and PrismaBuild action keys. It never copies file contents
or a transcript. A failed run reports the exception class without its message.
Summary-write errors do not change the worker reply or exit.

Worker retirement queues one bounded item per physical attempt in
`tl3_memory_outbox`, before removing the attempt's evidence. Missing summaries
fall back to the request, structured reply and supervisor exit. Interrupted
attempts are recorded as failures. Collection strips NUL bytes and checks the
item's size using Postgres's JSONB text representation. Invalid JSON and oversized records are logged and dropped. Store errors keep
the evidence and fail retirement so a later drain can retry.

A separate thread drains the outbox into `[hindsight].bank`, which defaults to
`tl3`. The bank must be separate from the coordinator's bank. The thread wakes
at startup and at controller drains through a channel of capacity one. An
outage leaves rows for the next controller wake or restart. It never polls on
a timer and never makes a request wait for Hindsight.

The server's `/openapi.json`, read on 2026-10-30, documents
`POST /v1/default/banks/{bank}/memories` with `async: true` and a client-supplied
`operation_id`. Repeating that UUID returns the original durable operation
without creating new work. The UUID includes repository, request, epoch, effect
key, generation and attempt. A stable document ID provides provenance. TL3
deletes its outbox row only after validating the server's durable acknowledgement.
Hindsight then owns extraction and its operation retries. This handoff uses
Hindsight's operation contract; it does not wait for extraction on the request
thread.

Authenticated servers use optional `[hindsight].api_key_file`. It contains the
raw API key in an owner-only file. The sender loads it at runtime and sends a
Bearer authorization header. A missing, insecure or unreadable file leaves the
outbox pending. It does not block daemon startup or request progress. Worker
requests carry only the file path in `hindsight_key_file`, outside model inputs;
recall reads it at runtime and sends the same header. The deployed service loads
`hindsight-key` through systemd's credentials directory. Neither the path nor key
is included in retained items. Status readers do not need the file.

A 401 or 403 stops the current pass and records one operations incident:
"Hindsight rejected TL3's key". The first successful retain clears that incident.
Another rejection can raise a new incident; repeated rejections update its HTTP
status without raising duplicates. HTTP 408 and 429 stay pending for retry.
Other 4xx responses park the row with its status for 30 days from parking.
Every flush deletes expired HTTP rows, including a flush with no pending rows.
Existing parked rows begin their retention period when migration 9 runs.
Transport failures and 5xx responses end the pass and leave rows pending for the
next wake. A hung endpoint costs one request timeout per pass.

The active retry set holds at most 10,000 rows; new records beyond that limit
park the oldest pending rows. After a successful retain drains the pending set,
the same flush requeues capacity-parked rows oldest first, within the cap, and
continues sending. Replay uses no timer. The status page shows pending, total
parked and HTTP-parked counts, plus any key-rejection incident.

After correcting a permanent HTTP error, operators can replay parked HTTP rows
with this single SQL statement. It preserves the active cap and operation IDs:

```sql
DO $$
BEGIN
    LOCK TABLE tl3_memory_outbox IN SHARE ROW EXCLUSIVE MODE;
    UPDATE tl3_memory_outbox SET parked_reason = NULL, http_status = NULL, parked_at = NULL
    WHERE sequence IN (
        SELECT sequence FROM tl3_memory_outbox WHERE parked_reason = 'http'
        ORDER BY sequence
        LIMIT (SELECT GREATEST(0, 10000 - count(*)) FROM tl3_memory_outbox WHERE parked_reason IS NULL)
    );
END $$;
```

Run it while the controller is stopped, then restart the controller to wake the
feed. The statement does not contact Hindsight.

Records have `repo:`, `request:` and `role:` tags. The controller selects summary
fields, omits infrastructure and repository file contents, removes credential
fields, and redacts bearer tokens, common provider keys, private keys,
credential assignments and passwords in URLs before enqueueing. Each item is
at most 32 KiB. Oversized sections carry a truncation marker. Pending reads use
64-row pages, and acknowledged items are deleted without a local receipt
history. Redaction replaces matched spans and preserves surrounding prose. Bare hex
strings of at least 32 characters stay redacted in prose. Public action IDs
are preserved in structured test activity. A maker reply's known 40-character
commit is retained as structured `head_sha`. No extra Git read is needed. Meanwhile,
generic input fields ending in `_key`, `_token` or `_secret` are removed.
Full transcripts remain the tracing system's responsibility.

`cargo test -p tl3-controller --test hindsight --locked` uses only a fake server.
It covers every discovered role, outages, lost acknowledgements, real SIGKILL
cuts, database restart, secret filtering, bounded records and cleanup. The
worker scenarios also prove that requests close during a memory outage and
leave their completed run records queued. Python's `test_memory.py` covers the
shared runtime collector for every role.
## Allowance looks and incidents

`looks` stores active allowances and incidents in `tl3_world`. It uses the
world-row schema. Store migration 6 adds the worker declaration of an outside wait.
`Controller::now` supplies every allowance time. Simulation can advance a fake
clock and call `look_due` without sleeping. `look_now` reads pending operations
after restart or a webhook gap.

The ingress receiver arms one timer for the next outstanding allowance. Expiry
reads the adapter's own record through `Executor::look`. The first live record
establishes a progress baseline. New checkpoints renew the allowance. An unchanged
record after the no-progress allowance establishes a stall. A read that cannot
reach its upstream is an infrastructure outage. Its allowance re-arms. These
reads use the same earliest-allowance timer, with no new polling source.
Finished deliveries leave the active set.

Worker looks read the supervisor and `tl3_worker_progress`. A stop intent is
stored before `stop_stalled` proves the exact process ended. A reply that raced
the stop is still used. Otherwise the kernel receives `RunFailed` and decides the
retry and incident. PrismaBuild alone decides whether its progress contract failed
or a worker was lost. A queue stall enters as `Fact::UpstreamStalled`; the kernel
raises its incident. The incident reads the merge time and queue-check title
and status from the watch's `WatchProgress`. Worker declarations of outside waits
are described in the [worker protocol](../../workers/README.md). A live declared
wait has no timer and stores no heartbeat or sample token. Its action key joins
the PrismaBuild source's bell awaited set. Endings request a look for that
effect. Bell resets reread the record and request a look only if it has ended or
errored. The subscription is durable until progress or retirement removes it.
The action's snapshot parent must be an ancestor of HEAD in the declaring run's
worktree. A wait whose action has unrelated lineage gives no liveness. Sibling
runs from the same base cannot be distinguished by this check. Git runs with
`GIT_NO_LAZY_FETCH=1`. Missing records give no liveness.

When a failed write's allowance expires, authoritative absence counts that
delivery as retryable through the kernel. A transport error never proves absence.
A new Found result, checkpoint or authenticated delivery releases holds only for
its own upstream. Progress, quota and stall observations release none. Duplicate
GitHub deliveries release GitHub holds. Repeated unchanged looks release none.

Read failures cannot acknowledge an unknown write. The controller persists their
failure count on its allowance and leaves that delivery pending under its original
key. Every second failed read raises an outage incident holding that delivery.
The first incident probes at each allowance until recovery or Rob's clearance;
later incidents require Rob. This preserves the failure escalation rule without
claiming that a transport error ended an operation. Incident-post failures retain
their delivery and allowance without recursively creating incident posts.
Stop and quota-resume errors are also logged, counted and re-armed. A persisted
stop intent keeps the exact attempt identity across restart.

Set `[github.operations]` to the existing pinned operations issue in TL3's own
repo. The app needs an installation there as well as on served repos. That
repository participates in authenticated comment intake and catch-up, but cannot
start a request unless it is also explicitly served. Incident messages use the
existing writer, checker, hidden markers and duplicate deletion. A real GitHub
adapter requires this section at startup, before reading credentials or opening
webhooks. Its constructor also rejects a missing target. Incident writer inputs
carry the original request title and link, plus the upstream and failure class
translated into plain language. They never use the operations issue as the request.

The deliberately narrow operations command is `clear incident <number>`, using
the number printed in that incident's message. Only Rob's authenticated comment
on that issue can clear it. A stale number cannot clear a later incident. This
command has its own translator before the request-comment arm and stores its
deduplication keys in `operations_comments`. Operations-issue comments never
enter `github_comments` or #14's classifier. Comments on other issues in an
operations-only repository are filtered during webhook and catch-up translation.
The PR that merges second must preserve that separation when combining #14. A passing adapter probe clears a first incident; the
kernel refuses probe clearance for a later one. `status::load_request_details`
loads all open incident summaries with one batched query.
The worker probe starts the configured Python and imports its worker package.
The GitHub probe reads the target issue. A failed operation's old ending does
not itself prove recovery. The default `Executor::probe` returns false.
PrismaBuild, Mergify and Worktrees first incidents cannot clear by probe until
those adapters implement it. This is the handoff for #67 and #69.
Rob can also close a permanent-failure incident
record; its separate retry-or-handle question remains to be answered.

When a GitHub allowance is due, or on `look_now`, a look also pages the App's delivery log with App JWT authentication.
Failed deliveries above the durable watermark request redelivery. A pending
delivery holds that watermark back. The watermark advances only after all
redelivery requests succeed, so a crash can repeat a request but cannot skip it.
The receiver's existing delivery keys deduplicate the recovered webhook.
On first start without a watermark, the controller records the current time before
reading GitHub. It skips retained deliveries older than that durable start time,
even if the first log read fails. New failures during that outage remain eligible.
An empty first page durably records ID zero. Later looks recover newer IDs from
the startup window. Existing ID watermarks retain their previous recovery window.

Quota is a wait for a provider-announced reset, never an infrastructure failure.
`Error::Quota` defers an uncertain write until the reset. A worker that receives
HTTP 429 with `Retry-After` or `X-RateLimit-Reset` emits a terminal quota record and
exits successfully. The supervisor proves that ending, and the controller resumes
under the same operation key and generation at the announced reset. Malformed
or absent reset information remains an ordinary failure. No provider text enters
the protocol. Other adapters can return `Look::Quota` or `Observation::Quota` and
implement `resume_after_quota` for acknowledged operations. A zero reset remains
due immediately, so a zero `retry_after` cannot strand a quota wait.
The supervisor tracks the attempt as well as the key, so retirement of the quota
ending cannot remove a resumed process from its active set.

From a clean detached worktree at the final commit, generate the host runs,
workspace checks, worker protocol tests, kernel tests and stall proof with `python3 crates/controller/tools/looks_manifest.py`. Submit the
manifest with the published `pbcampaign.py`. Process and timing tests run once
on each of dl380g10, sparky and sparklina. Check every terminal record, log and CAS
receipt before reporting a pass.

## Failed notice posts

A permanent writer rejection or GitHub refusal finishes the delivery as Failed.
The same transaction retains its payload in `notice_failures` and raises an
incident on the operations issue, naming the request, notice kind and error class.
Status says when an open question was never posted. A failed incident post
updates status and emits no further post.

Rob can reply `clear incident <id>` on the operations issue after fixing the cause.
A healthy adapter probe can also clear the first incident. `retry_notice` creates
a fresh delivery and writer attempt with the original `Delivery.marker_key` for
GitHub reconciliation. Clearing and scheduling the retry share one transaction.
If the retry fails permanently, the later incident requires Rob and probes cannot
clear it. Confirmation deletes the incident counter for that original marker.
The kernel world, question ID, continuation and credit stay unchanged. The
classifier continues to offer the original question's answers. If that question
is no longer open when the incident clears, no retry is queued.

A closed request stays closed while its summary, note or correction filing
retries. A correction filing can start a new linked request. The dropped-change
payload retains Rob's text after its replacement row has been deleted, and the
retried note includes that text as an indented quote.

Generate ticket 87's host actions with
`python3 crates/controller/tools/notice_review_manifest.py hosts`.
The GitHub target includes notices, steering and retry crashes at every store
and adapter save point, including question 403 and incident-post failures.
The host actions also run looks. The `workspace` selection runs `tools/verify.sh`.
Use `all` to generate both sets. These commands run through PrismaBuild.
The kernel and state table match `main`, so this controller change needs no
Kani campaign.

## Review and publishing

The supervisor maps `Role::Review` to the `reviewer` worker. It selects the first
model returned by `models.reviewer_for` for the current maker attempt's family.
A missing independent family blocks dispatch. Review runs in its own process and
reads a private clone at the exact checked commit. Dirty maker files never enter
that clone.

The controller retains each check's tests, outcomes and `custody: in-process` in
`check_reports`. Each node keeps only its latest head's report, and closure deletes
it. A review needs the matching check plan, base and report. Its diff is capped at 512 KiB. Goals receive stable `goal-N`
identifiers in the accepted `goals` document when ingest did not supply IDs.
Maker inputs and reviewer criteria share the same ID function.
Optional `standing_rules` in worker materials supplement the controller's rules. The reserved `test-report-integrity`
rule always replaces any supplied rule with the same ID. It protects test output
and pytest internals while permitting ordinary program output. Review blockers
must cite a supplied goal or rule. Approvals must name the exact reviewed head.
The controller carries blocking findings into planning.

Each reviewer attempt uses the maker's session directory and atomic writer,
with a distinct file named `reviewer-attempt-N.json`. The re-plan directory keeps
attempt 1 of a new plan separate from attempt 1 of the old plan. Round 1 resumes
its own conversation, including model reasoning and tool results. Its new `diff`
starts at the last reviewed head in `base_sha` while that head contains the
current check base. After a rebase moves the check base beyond that history,
`base_sha` uses the new check base, so the diff contains only the leaf's change.
`check_base_sha` names the recorded commit used for the check results' base
outcomes (`CheckRun.base_sha`), including the new target after a clean rebase.
If round 0 failed checks and never reached review, round 1 starts a fresh
conversation.

The controller records each accepted review's generation, attempt, re-plan flag
and head in the existing worker document store. A retry before acceptance keeps
the same visit across generations. A retry with inputs that differ from the saved
session fails on purpose: reusing its reply or pending reads would mix evidence
from different heads or checks. The worker checkpoints model turns and tool
results before emitting progress. Recovery executes pending reads without another
model turn. It validates and saves its final reply before emitting completion.
A read interrupted before its result was saved may be repeated. A completed visit returns
its saved reply without appending the input or calling the model again. A later
accepted recheck is a new visit, even if its evidence is unchanged. New attempts
start fresh. Closing the node deletes reviewer files with the maker sessions
through the existing retirement path.

The reviewer uses the maker's 85% context guard, including provider usage counts
and the character-count fallback. The controller supplies `context_size` from
its limits; direct requests without it use 150,000 tokens. Exhaustion fails the
worker without a verdict. The session records exhaustion before failure, so a
retry does not repeat the model turn. Bounded rounds do not bound tool reads
within a visit. No new kernel reply or transition is needed.

To reproduce #88's checks, generate a manifest with
`python3 crates/controller/tools/reviewer_resume_manifest.py hosts` or `workspace`,
then submit it with `pbcampaign.py`. The `red` option generates the focused
pre-fix scenarios; set its row's `cwd` to a clean detached worktree at the red
commit before submission. The host rows run the Python workers,
publishing and workers scenarios on each named host. The workers scenarios include
the maker scenarios. The workspace row runs `crates/controller/tools/verify.sh`.

`github::Adapter` executes `PostApproval`, `OpenPr` and `ClosePr` through the App
client. The check is named `TL3 approval`. Its external ID contains the effect key
and exact SHA. Recovery requires a successful completed check from that App on
that SHA. PR recovery reads all open and closed PRs for the request's owned branch.
Opening an existing unmerged closed PR reopens it. Closing an absent, closed or
merged PR confirms without another write. Transport failures remain unknown.

The kernel sequences push, approval, PR opening and queueing. Mergify adds the
queue label only after those steps. A new commit or clean catch-up receives fresh
checks and review. Outside pushes retain the request's check plan. Clean rebases
carry that plan onto the reported new base. The worktree adapter performs rebases and removes
the worktree and branch during teardown. The GitHub message pipeline posts the
closing summary and closes the issue.

After closure, PrismaBuild retirement requires every check effect and watch to
be terminal. `retire_node` also reads each unfinished binding and refuses a
Queued or Claimed upstream row. Corrupt bindings and failed reads retain records.
Retirement continues with other closed nodes after a failure, then returns the
first error. Successful retirement removes the request's directory and notifies the source.
The configured submission tool must be `pbrun` itself or a wrapper that `exec`s it,
so the controller's death signal reaches the publisher.

To add a supervisor role, update these points together:

1. Map the existing kernel role in `workers/protocol.rs`.
2. Add its input and reply JSON schemas to the workers package. The controller
   embeds every packaged schema and validates the registry at supervisor startup.
3. Assemble its input, model and committed worktree in `Supervisor`.
4. Validate input-dependent evidence in `workers/source.rs`.
5. Translate and persist the reply in `Controller::translate`.
6. Exercise the process and its retirement on all three fleet hosts.

Generate the development campaign from a clean commit:

```bash
python3 crates/controller/tools/publishing_review_manifest.py hosts > /tmp/tl3-publishing.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-publishing.json
```

This runs the publishing, GitHub and PrismaBuild adapter scenarios once per host
on dl380g10, sparky and sparklina. It also runs the affected Rust suites,
including process recovery scenarios, and the worker package's offline tests. The
`publishing` target combines local repositories, mock GitHub HTTP, recorded
PrismaBuild records and recorded Mergify check fixtures. It runs the reviewer
entry point with an offline model. Publishing scenarios use the fake maker to
control commit and check evidence while exercising review, publication and cleanup.
The maker scenarios separately run the real maker process. Publishing covers
merge and cleanup, an outside commit, a free catch-up recheck,
an unapproved merge, and lost-response recovery for GitHub writes. No test submits
nested fleet work. After the host runs pass, generate the `workspace` campaign
and submit it for formatting, Clippy and full workspace verification.
