# TL3 controller

`tl3-controller` connects the pure kernel to the Postgres store and upstream
executors. Read [the architecture note](../../docs/architecture.md) for the
transaction and recovery contracts.

Service commands use the same TOML configuration as `tl3-config`. The daemon runs
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
findings on every subsequent attempt.
The maker sees a bounded view of that history (#316). A finding that a later review or
check superseded is sent as a one-line record: kind, id, title and `resolved_round`, the
count of feedback events (a review or a check failure) up to the event that superseded
it. The latest review, the reviewed head's findings and an unsuperseded check failure keep
their full text. `current_review_findings` lists ids only. The store keeps every finding,
and the reviewer reads the full history.

Supersession is positional, by the append order of `maker_findings` (`lib.rs` only
pushes to it):

- A newer check failure collapses the older ones.
- A newer review collapses the findings of older reviews. That is safe because the
  reviewer keeps the full history and re-raises anything still wrong.
- An approving review adds no findings, so it collapses nothing. Ingress supplies the brief slice and
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
transaction as its kernel fact. Merge queue failures read the failed draft's
checks and bounded job-log tails through the GitHub App. When the queue check
names several draft attempts, the collector uses the last one. Feedback includes check
names, conclusions, titles, summaries, details URLs and parsed Bazel or pytest
test IDs. ANSI escapes and control codes are removed before parsing and
redaction. External text is clipped to 20 tests,
1,000 characters per message and 16,000 per report.

When every failed check supplies parsed failing tests, every test maps to an
unchanged file, and the plan named none of the failing tests or checks, the controller journals an infrastructure
failure with those IDs. The kernel requeues once without a maker or credit.
A repeat on any of those tests takes normal failure handling with evidence,
including after restart. Unknown mappings, absent IDs, unavailable plans,
truncated logs and other incomplete evidence take normal handling. Recorded
dequeues skip draft, job-log and diff collection on redelivery, including after
restart. A bare non-infrastructure CI fact still records feedback
stating when the upstream supplied no check details. Infrastructure failures
and stale replies do not add maker findings.
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

The host actions run maker, worker, supervisor, PrismaBuild adapter, GitHub
ingress and worktrees scenarios. They also cover process exit during both
opening and reading `/proc/<pid>/stat`.
The open case extends main's ESRCH fix; permission errors still propagate.
The daemon fixture makes and commits a real change through the shared ingress loop.
A separate portable action runs the Python worker and harness suites and
`tools/verify.sh`. These tests use fixtures and never submit nested jobs.
`tools/maker_trial.py` runs a new real maker-to-harness trial in its own
admitted action. The image must already exist on an eligible worker. The script
requires admission and never submits work.

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

`Webhooks::bind` starts receipt immediately, before controller recovery or source
registration. Until registration, receipt retains one pending wake bit; it then
forwards wakes directly to the controller source channel. Catch-up reads its snapshot through a separate read-only connection
and fetches GitHub evidence on its own thread. The receipt
loop remains the only ingress writer and commits each completed catch-up batch
and its cursor together. At most one fetch runs at a time; gaps during a fetch
request another pass after that batch commits.

Source registration starts the first catch-up. Ticket #19 wires the tunnel supervisor
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
options. Closing summaries close the issue, except when the request is replaced or
stopped. A stopped request's summary says to comment `restart` to start again
(Q49).
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
`workers.launcher`, when set, starts each attempt's supervisor through that
command. The deployment uses `systemd-run --user --scope` under
`tl3-workers.slice`, so a tl3d restart or deploy, despite its
`KillMode=control-group`, leaves runs alive (#372). The restarted controller
reattaches through the claim record and accepts the reply under the same key
and generation. A launcher failure fails the launch. The supervisor launches
the Python that `workers.python` names after resolving its directories, and
records it in `launch.json`, so a later release switch leaves a running worker
on its own code. Recovery reconciles durable records before retrying.
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

The review-fix evidence adds the kernel tests and three Kani harnesses covering
retryable acknowledgements, publishing transitions, and queued progress. Read
each terminal record, log, and CAS receipt before reporting a pass.

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

The Mergify checks run controller, GitHub, Mergify and store tests, including the
crash simulation smoke run. The scenarios use local HTTP servers and embedded Postgres.
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

The adapter review runs controller, client and store process tests.

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
default `merge` smoke scenario covers all twenty save points. Set
`TL3_SIM_FULL=1` for the full matrix and effect-and-phase coverage assertion.
Its seed and optional scenario and save-point filters appear in failure output.
The receiver and status review runs the controller, GitHub and Postgres test
targets. Controller targets include the status page and process tests.


## Worktree adapter

`worktrees::Real::new(&config.worktrees_root, epoch)` uses the existing configurable
root. Add each repository with `serve_github(identity, local_repository, github)`.
The GitHub client must match the repository's installation. The client supplies
cached installation tokens at runtime. For simulation, use `serve_local` with a
local bare remote. Local operations never obtain or pass a token.

Register the adapter once with `registry.register(Upstream::Worktrees, real)`.
The daemon still requires explicit simulation; production ingress and startup
registration belong to their adapter tickets.

The adapter uses each repository's `branch_template`, defaulting to `tl3/issue-N-node-M`.
Configure both adapters with `with_repositories(&config.github.repos)`. An unconfigured
repository returns an error before Git or publication. Checkout paths beneath the
configured root separate repositories and requests. `Real::branch_for` and
`Real::checkout` expose those names for worker registration. One controller
process and its Git children own each repository. Stop the whole service process
group before starting a replacement. Do not run another Git writer or maintenance
process against an owned repository.

Run `bash crates/controller/tools/verify-repo-conventions.sh` through PrismaBuild
to exercise template round trips, config rejection, publishing and rebasing onto
`master`, and cleanup. The script evaluates the published PR against the offline
PrismaBuild issue-link fixture fetched from its repository; `provenance.json`
records both policy and workflow blob SHAs. Catch-up, steering, adoption and
status identify existing work through stored request and PR bindings; they do
not infer ownership from a separate branch grammar.

Keep `<common-git-dir>/tl3-controller` across restarts. It binds the repository
and worktree root, advances the epoch fence as soon as the repository opens, and journals immutable
operation intents and terminal evidence. Changing that root requires an explicit
migration of existing checkouts and records. Records use atomic replacement and
sync both the file and its directory. No store migration is required.

Prepare pins the fetched `base_branch` before creating missing resources. Git's administrative
lock `<common-git-dir>/worktrees/<id>/locked`, with the exact reason `initializing\n`,
is the evidence of interrupted checkout population. Any key may repair it only
while the branch head equals the durable base ref. Repair runs
`reset --hard <branch>` and unlocks after verifying the files, even if the base branch has advanced.
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
call `branch_state(&real.branch_for(delivery)?)` to read its recorded base and current
head. The tuple is `(head, base)`. `changed_files(&base, &head)` returns the changed
paths for the check harness and publication footprint. After a clean rebase,
read both values again: the base and head move together. `Delivery::head_sha`
remains the exact commit requested by an individual push effect.


The worktrees checks run all controller and worktrees targets, including the 50
SIGKILL boundaries and real killed-add recoveries. See the [worktrees recovery guide](../worktrees/README.md#recovery)
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
per test process. The process scenarios include controller and store process
scenarios.

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

The steering checks run the GitHub target, including steering races, and the
Python writer tests. Inspect each terminal result, logs and CAS receipt. No
kernel rule or proof changes.

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

`question_context.findings` carries the deciding worker's evidence to the
question writer (#322). It comes only from the step that raised the question.
A source that does not match the cause gives no findings, never stale ones.

- **AuditDisagreement:** the audit's objections, with the reading and goals, or
  an early end's outcome and reason.
- **NeedsSplitting:** the planner's split, with its size cause and pieces.
- **CantMeet:** the planner's report, rejection reasons or size estimate; or the
  maker's report or the failures this run got.
- **OutOfCredit:** after a failed round, that round's failures. Running out of
  room, used-up rechecks and a rebase that needs changes each state that cause.
- **EffectFailed:** the incident the failure raised.

`decision` is the worker's own question: the report's last question mark that
ends a sentence. Otherwise it is a fixed question for the kind. Reports are
clipped to 6,000 characters, and pieces, files and items to 600 and to eight.
The writer worker redacts findings with the MLflow redactor (#302), including
the run's loaded model key, before the model turn. A question with findings must
state its decision as one question. The obstacle is capped at 600 characters and
the decision at 400. Explicit planner and maker refusals carry
`outcome: "cant_meet"`. Their questions must name at least one reported path or
rule in the obstacle. Those exact paths are allowed in that question, while
other paths and internal vocabulary remain subject to the usual checks. The
writer explains the worker's proposed decisions within the three standard
answers, including any required permission and a smaller request. When the report
names no path or rule, the obstacle must share a distinctive word of at least
five letters with its first sentence. When the report proposes an action, at
least one option must name a reported blocker or proposed action.

If every draft is rejected, the controller still posts a fixed question. For a
can't-meet report, it redacts secrets and clips the first sentence naming the
blocker to the obstacle's limit. If that sentence fails the message checks, the
controller uses a fixed sentence naming a safe reported blocker. Unsafe details
cannot suppress the question or bypass the message checks.

Run `bash crates/controller/tools/verify-planning.sh` through PrismaBuild once
with each host tag, `dl380g10`, `sparky` and `sparklina`. It runs controller and
store process scenarios and the worker package's offline fake-model tests.

The planner process scenarios cover malformed output, a controller killed while a
planner is running, and stopping a live planner. Malformed replies fail without
storing a plan. Recovery reattaches to the same process and key with one attempt.
Stop proves the process has ended and stores no plan. The leaf, split, rejected
leaf, invalid split and cannot-meet scenarios also check persistence, narrowing
context and retry findings.

## Postmortems

Q32's postmortems are pulled forward (#403). `postmortems.rs` records them
outside the kernel, in the commit that makes them owed, so a crash can't lose
one and no delivery work waits on one.

- **At close**, every node gets code grades under TL3's rubric
  (`src/rubric.json`, version 1): reliability, escalation and queue outcome are
  graded 1 to 5, and credit, questions, incidents and time per phase are recorded
  as facts. The kernel zeroes its failure counters when a step succeeds, so the
  world row `retries` counts each run, effect and plan retry in the commit where
  its counter rises; reliability and selection read it. Time per phase is in
  milliseconds from the phase history; the phase that ends at close is left out,
  so the grades depend only on stored times. A postmortem is owed when the node asked Rob or raised an incident
  that needed him (`escalation`), had a fix round, later attempt or replan
  (`needed_fix`), had a run, effect or plan retry (`retry`), or closed without
  merging (`closed_without_merge`). Every clean merge also owes one (#642). Its
  stored reason remains `sampled` with a share of 1 for existing readers. The
  legacy `postmortem_sample_share` setting no longer selects work.
- **Every question to Rob** owes one (`question:<id>`), and so does every kernel
  incident that needs him (`incident:<id>`).
- **Rob's console button**, `{"kind": "postmortem"}`, owes one (`rob:<command
  id>`, `rob_asked`) on a closed request or one awaiting Rob. It's refused while
  one is owed or running, and posts nothing on the issue.

Early-end closes (duplicate, already done, not actionable) stay owed: a wrong
early end is exactly the quiet error a postmortem should catch. Their number is
bounded by closes, questions and incidents, and `tl3_phase_history` grows by one
row per phase change.
The store tables are described in `crates/store/README.md`.

### The runner

The runner judges owed postmortems with the workers' `postmortem` role (slice B
of #403). Like comment classification, it is a worker job outside the kernel:
it holds no pool grant, and no delivery work waits on it.

- **When.** tl3d calls `run_postmortems` as it starts; a controller without it,
  such as a test whose workers run no postmortem role, leaves owed rows owed.
  `drive_once` then calls `start_postmortem` on every pass, so a postmortem
  starts when a commit makes room, never on a timer. The lane has two slots,
  independent of delivery's agent pool. Pool holders and delivery waiters never
  block it, and running postmortems never consume delivery grants (Q39 amended
  by Rob on 2026-11-03, #642).
- **Quota and recovery.** `start_postmortem` asks the store for an owed row first.
  Entry 0 writes the postmortem and entry 1, if present, grades it. Each falls
  down the whole ladder past models whose `model_quotas` record hasn't expired,
  as other roles fall back (Q53, #759), and the lane claims nothing only while
  every entry is out. There is
  no daily allowance. Three consecutive failures pause new claims. Recovery
  resets the failure boundary and retains every claim and outcome for audit.
  A single validation run must succeed before the lane resumes; failure pauses
  it again (#632). The existing deadline still applies.
- **Claim.** The oldest owed row, including retries, becomes `running` with a `run_key`, which is
  also its delivery's key (`postmortem:<id>:<revision>`), in the commit that
  writes the delivery: its effect, its watch, its allowance and its
  `postmortem_jobs` row. `PostmortemUpdate::Claim` fails that commit if the row
  is no longer owed.
- **Run.** The workers adapter launches the role like the classifier and keys
  its active run apart from the node's, so stopping a node's run never stops its
  postmortem. Its looks read progress like a node's run, and a postmortem raises
  no incident. The case file is written at launch from stored records: the
  request and outcome, the code grades, phase times, questions with their #322
  findings, incidents, plan, brief, audit and findings, the release tl3d
  started with, the rubric, the grade judges' model, the node's MLflow tags and
  the run's wall-clock allowance (`limits.postmortem_deadline_secs`).
- **Result.** The worker's reply is checked against the role's schema and, in
  code, for at most three recommendations. It is stored only for the claim's
  run key, in the commit that consumes it: the row becomes `done` with its
  timeline, cause, summary, process failures, grades and assessment ids; its
  recommendations are ranked and fingerprinted; tokens and cache rate are later
  code grades (`postmortem:<id>`); and the lessons go to `tl3_memory_outbox`,
  tagged `postmortem`. Stored and retained text passes through the memory
  redaction first. A failed run, a refused launch or an invalid reply marks the
  row `failed` with a fixed class: `worker_failed`, `launch_failed`,
  `invalid_reply`, `no_progress` (the controller stopped it as stalled) or
  `deadline` (the run passed its allowance). A `worker_failed` or `deadline`
  row is owed again instead, twice at most, behind rows owed since (#486).
  Three failed runs in a row since startup or the latest recovery pause claims
  and raise one operations incident. Clearing that incident resets the failure
  boundary in the same commit, without restarting tl3d. The first start under a
  new release clears this TL3-side pause through the same path, including pauses
  recorded as upstream incidents by older releases.
- **Recovery.** At startup, a `running` row whose delivery isn't live (no pending
  effect, watch or queued result) is owed again, and a `postmortem_jobs` row
  with no live delivery is removed. A live delivery finishes as it would have:
  the worker's attempt directory holds its reply, and the role caches each step
  under the run key, so no judge runs twice.
  Clearing or recovering a pause also persists one validation permit. The next
  eligible claim consumes it atomically with the delivery, even if the daily
  allowance is exhausted or zero. It uses the same worker deadline and allowance
  as an ordinary postmortem. Its claim event records `postmortem_validation`, and
  the claim ledger records its outcome. A successful validation resumes ordinary
  pacing. Any validation failure, including a permanent launch failure or an
  orphaned claim released at startup, immediately raises a new pause incident.
  A restart preserves the permit and its running or failed validation state.
  Recovery does not bypass model quota or Q39 and reserves no pool slot.
- **Blocking reason (#632).** `tl3 status` and its HTML view expose
  `Status.postmortem_blocked`, a typed `PostmortemBlock` from the last scheduler
  pass. The first blocking gate is `paused_streak`, `daily_allowance`,
  `q39_waiting_delivery`, or `no_spare_capacity`. Model quota reports
  `model_quota`. A running postmortem uses the runner's only available claim
  slot. The reason clears when an eligible claim starts or no owed work remains.
  Repeated passes at the same gate write no additional events.

Nothing in slice B acts on a verdict.

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
`tl3`. The bank must be separate from the coordinator's bank. Startup and
controller drains send coalesced channel wakes. After a delivery error the
thread holds the queued rows, ignoring ordinary drains. There is no private
SQL listener or feed retry timer.

The controller records an upstream incident, `Hindsight unavailable: N memories
held`, and posts it through the existing operations-issue incident path. Other
requests continue. Restore Hindsight or its credentials. The controller's
existing earliest-deadline look schedules a probe after two minutes, matching
#759's unavailable-model probe. The probe requests delivery with the saved
operation UUIDs. A deadline does not clear the incident: acknowledged delivery
and durable queue removal do. Failed probes rearm the same look. Recovery drains
the queue and clears the incident automatically, without an operator command.
`tl3 memory retry --config FILE` requests an earlier attempt through durable
retry evidence and the existing ingress wake. Evidence arriving during a failed
response survives that failure. A successful retain or curation from another
process also requests an attempt. Restart retries the durable queue.

Web status and `tl3 status` show `Feed` (`ready`, `delivering`, `held`, or
`failed`), `Pending`, `Parked`, and `HTTP parked`. A lost consumer shows
`failed (no consumer; restart tl3d)` and raises a TL3-side incident. Retry then
returns an error instead of reporting success. A session advisory lock proves
consumer presence without a heartbeat. Dropping the final supervisor handle
sends a local stop and joins the thread before dependencies are released;
shutdown needs no new database connection. The feed retains one writer session
while idle and has no blocking SQL listener.

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
outbox pending and never blocks request progress. The deployed unit loads the key
as a systemd credential, so the file must exist for `tl3d` to start;
`deploy.py check` reports it as the Hindsight credential. Worker
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

Memory incidents follow Q7 by operation lineage. Retains use the stable operation
UUID; curations use their durable sequence. A successful probe can clear the
first incident. Only Rob can clear later incidents for that lineage, using the
`clear incident ID` command on the operations issue. Probes and `memory retry`
leave those operations held until clearance. Clearance is checked before each
outgoing operation, so a preceding curation cannot release a retain's hold.
Rob's clearance covers the current persisted outcome. Other operations get their own
incident count. A reused incident after restart always rearms its recovery look.
Feed failure or panic wakes the existing worker ingress source, even if the
source registers after the exit. Intentional joined shutdown sends no failure wake. Feed failure does not skip worker-session retirement.
Migration 19 adds a runtime version marker so clearing an incident does not
count the same failed attempt again. Migration 18 stays unchanged.

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

## Hindsight recall and curation (#422)

**Recall.** `Supervisor::request()` calls `memory::recall` before every ingest,
planner and reviewer launch, so recall is the first thing those roles see.

- Ingest queries the repo, issue title and body. The planner queries the repo,
  brief and goals.
- Both get lessons, decisions and conventions first, then any other memory.
- The reviewer gets only `kind:convention` memories tagged with its own repo.

**Cross-repo policy.**

- Ingest and planning recall is not filtered by repo. Each query names the repo,
  so same-repo memories rank first, but a lesson from another repo can still be
  supplied.
- Every supplied memory carries its `repo` tag, if it has exactly one.
- A run can hide or retire only memories tagged with its own repo. A memory with
  no repo tag can't be retired by any run.
- Rob can retire any memory with `tl3 memory`.

**Bounds and headroom.**

- At most 8 memories, 1,500 characters each and 8,000 in total. That is about
  2,000 o200k tokens of English, inside the planner's and reviewer's context.
- The forecast sizes the maker's context, and the maker gets no supplied
  memories; its read-only `recall` tool is bounded separately. Supplied memories
  therefore change no forecast.
- The controller redacts memories with `safe_text`. The worker then applies
  #302's redactor before any model turn.

**Outages.**

- Any failure gives the run `memories: []` and a `memory_note`. It is never an
  incident or a wait.
- One failed recall opens a breaker, so later launches skip recall without
  contacting Hindsight. The breaker closes when Hindsight next acknowledges a
  delivery, such as the next retained run record. Then the next launch tries
  again. No timer is involved.
- Retain can succeed while recall still hangs. Then each acknowledged retain
  closes the breaker, and the next launch pays one recall timeout (5 s)
  before it opens again. The cost is bounded by the retain rate, which is at
  most one per retired worker run.

**Wrong memories.** Ingest and planner runs read a pinned base checkout, so
their flags are evidence about the repo. Each flag must:

- quote the memory's claim verbatim from its content;
- quote the repo, at most 40 cited lines, sharing an identifier with the claim.
  An identifier is a name only code uses: at least 5 characters, with an
  underscore, a digit, internal mixed case, `::`, `.` or `/`. A plain word like
  `database` or `error` matches too many lines to tie evidence to a claim.
- come from the file the memory names, when it names one by path or
  extension.

The flagged memory must also be cited, tagged with the run's repo, and not a
decision. Code checks every condition at run retirement against the run's own
worktree. A rejected flag is kept with its reason and changes nothing.

`tl3_memory_curations` (migration 14) is the trail for every decision:

- One accepted flag records `hidden`. TL3's recall drops the memory at once, and
  Hindsight is not told.
- A second accepted flag from a different request, or `tl3 memory confirm`,
  records `invalidated`. The feed then sends `PATCH …/memories/{id}`, and
  Hindsight retires the memory everywhere.
- `tl3 memory invalidate` retires a memory on Rob's word. `tl3 memory revert`
  sends `valid` and clears both states.
- `tl3 memory list` prints the trail.

The feed posts one line per hide and retirement on the operations issue. The
line holds the memory id, the claim, the evidence, the source and the run. A
per-row marker keeps a retried post from duplicating.

Retained lessons carry `repo:`, `kind:` (`run`, `lesson`, `decision` or
`convention`), `source:` and an optional `topic:`:

- Rob's applied answers become decisions holding the question, his choice and
  his words for that answer.
- Verified planner conventions become conventions.
- `memory::queue_lesson` is the entry point for postmortem lessons (#403).

`cargo test -p tl3-controller --test hindsight_context --locked` covers this
with a fake bank.
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

Mergify recovery probes read the PR issue with one GET through the existing
incident allowance. A successful read releases the hold on a quiet upstream.
A confirmed effect resolves its recovery incident. Closing a request resolves
its remaining operation recovery incidents; restart also repairs older closed
rows. Failed notices still need delivery after closure. Kernel warnings,
including unapproved merges, retain their own lifecycle.

Cancelling queue admission ends its local watch without waiting for Mergify.
The synchronous dispatch has ended, and the existing ClosePr teardown removes
the queue label and closes the PR to undo any accepted write. This cancellation
uses the original effect key across restart and does not claim queue acceptance.

Mergify command comments have a separate reply. The adapter matches that reply
to the App's marked command, requires Mergify's bot identity, and ignores old
replies or replies following someone else's command. A refusal immediately
raises a setup incident and is journalled across restart. That incident stops
automatic recovery probes because a healthy GET cannot prove that command
restrictions were corrected. Fix the served repo's command restrictions and
clear the incident before retrying; the command receipt alone never proves
queue acceptance.

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

Q53 moves a quota-limited worker step down its role's ladder. The quota
observation names the model, and the controller records it in `model_quotas`
with its reset. The look then asks the adapter's `quota_resume_ms` when the step
can resume: now if another ladder model is free, otherwise at the earliest
reset. The relaunch picks the first model that hasn't reported quota. A reset
deletes its `model_quotas` row. The startup look asks again, so a wait for a
model no longer configured ends when tl3d opens. A wait recorded before Q53
names its model only in the attempt's `request.json`, which the adapter reads.
The maker's reply records its model in `maker_session`, and the reviewer stays a
different family from it.

When every model a step may use is out, and has been for 30 minutes since the
step's first ending with no model turn served since, the look posts one
operations note with the fixed reason `model_ladder_unavailable` and keeps
waiting (#759). The wait wakes at that bound if it comes before the earliest
reset. A progress checkpoint clears the step's record, so a later outage posts
again. A postmortem's quota event may name its case file's grade model, and the
controller charges that model; its grade judges run on the first model not out
from the ladder's second entry on.

The looks checks cover the workspace, worker protocol tests, kernel tests and
the stall proof. Check every terminal record, log and CAS receipt before
reporting a pass.

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

A restart (Q49) reads the issue's current title and body through a `restart:`
GitHub effect, then starts the new request through `replacement_pending`. The
effect reopens the issue only when TL3's app made its latest close. A 404 or 410
means the issue is gone: the restart is dropped, nothing is filed, and a plain
note goes to the operations issue. Any other permanent failure raises the usual
notice incident and keeps the restart pending; clearing the incident retries
the read.

The GitHub target includes notices, steering and retry crashes at every store
and adapter save point, including question 403 and incident-post failures.
The host actions also run looks.
The kernel and state table match `main`, so this controller change needs no
Kani campaign.

## Review and publishing

The supervisor maps `Role::Review` to the `reviewer` worker. It selects the first
model returned by `models.reviewer_for` for the current maker attempt's family.
After the worker reports exhaustion, Q44 starts a fresh conversation on the next
reviewer model. The coordinator's ladder clamp and restart defaults are listed
below. A missing independent family blocks initial dispatch. Review runs in its own process and
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
its limits; direct requests without it use 150,000 tokens. The worker records
exhaustion and returns `out_of_room` with exit zero. Recovery repeats that reply
without another model turn. A model-emitted `out_of_room` is rejected; only the
worker's context guard can report exhaustion. The controller maps it to
`ReviewerOutOfRoom`. The first exhaustion starts a fresh conversation on the next
reviewer model. A second exhaustion requires Planning to split, using the maker's
out-of-room credit rules (Q44).

Coordinator defaults, for Rob to check: the model ladder clamps at its last
entry. The first restart shares the re-review allowance, free up to
`free_rechecks`, then 1 each. A clean rebase or outside push to a new head resets
the restart flag and keeps the recheck counter. Ordinary fix rounds continue the
selected session; new maker attempts reset the reviewer. The controller carries
the latest accepted generation even when starting a fresh conversation, so an
older exhausted checkpoint is not treated as an unaccepted retry.

The #88 checks run the Python workers, publishing and workers scenarios. The
workers scenarios include the maker scenarios.

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
closing summary and closes the issue. It leaves the issue open after Replaced or
Stopped (Q49).

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

The publishing checks run the publishing, GitHub and PrismaBuild adapter scenarios.
They also run the affected Rust suites,
including process recovery scenarios, and the worker package's offline tests. The
`publishing` target combines local repositories, mock GitHub HTTP, recorded
PrismaBuild records and recorded Mergify check fixtures. It runs the reviewer
entry point with an offline model. Publishing scenarios use the fake maker to
control commit and check evidence while exercising review, publication and cleanup.
The maker scenarios separately run the real maker process. Publishing covers
merge and cleanup, an outside commit, a free catch-up recheck,
an unapproved merge, and lost-response recovery for GitHub writes. No test submits
nested fleet work.


## Proposed-head scope hook

Before reporting a leaf's proposed head or starting its first check, call:

```rust,ignore
controller.trim_scope(&worktrees, &delivery, &config.planning.protected_paths)?;
```

The adapter adds the mandatory defaults and the `github.repos` protected paths
matching `delivery.repository`; callers pass only the global additions.

`delivery.paths` contains the plan's declared files. The trim runs once per branch.
Its receipt always returns the original trimmed head, even after later maker
commits. The adoption tracer must re-read the current branch head after the hook
and report that head through the existing observation. A replay never resets
later work. The hook does not change
kernel state, approve a head, or run a maker. It has no production caller yet;
the adoption tracer can call it when it supplies proposed heads.

The worktree adapter fences the trim against its durable epoch. The controller
then records the head and removed paths in a `scope_trims` world row. Repeat the
same call after a crash between Git completion and the SQL commit. Both stages
are idempotent. The closing summary carries the removal text in the writer's
`facts.removed_changes`, supplied by `Delivery.scope_removals`. Saved-scratch
text keeps its separate `Delivery.note` field and its separate fixed append.
TL3 appends removal text after checking the writer's prose, so the
existing checker continues to reject file paths in model-written text. Path
bytes outside letters, digits, dot, underscore, hyphen, and slash are percent
encoded to keep filenames from becoming references or Markdown instructions.
Each listed path is inline code to prevent emphasis and automatic links.
The removal notice is capped at 8 KiB, leaving room in the GitHub comment for
the writer's prose and scratch notice. It lists complete paths in order, then
counts the omitted paths. The durable row retains the full list.

Review always supplies the `request-scope` standing rule alongside
`test-report-integrity`. A scope blocker must cite that rule, quote a changed
hunk, and explain why it serves none of the supplied goals. On a fix round,
`scope_diff` supplies the whole remaining change against the check base, while
`diff` keeps the existing incremental review behavior. Identical diffs are sent
only once. Scope findings quote a whole changed line from the unrelated hunk
with at least 8 non-whitespace characters of changed content. Supporting imports,
fixtures, documentation, type hints, required formatting and needed small helper
refactors are in scope. Uncertain scope concerns stay non-blocking.

The scope checks include the worktrees suite and controller scope scenarios,
reviewer and writer Python tests with the deployment helper, and `tools/verify.sh`.

### Required head checks before queue retry

The enqueue adapter reads the union of the base branch's ruleset and classic
branch-protection requirements. A classic-protection 404 means no classic requirements.
It waits for every required check on the approved head before queueing, and matches
each required check's name and App ID against the latest check on the approved
PR head. Cancelled, timed-out, and startup failures from GitHub Actions rerun their
workflow. Enqueueing waits for a successful rerun. A `failure` needs explicit runner-failure annotations to take
that path; ordinary test failures enter the existing fix round. A generic GitHub
merge refusal uses the same evidence. Optional checks and older heads are excluded.

Each rerun records its run ID and attempt before the POST. Recovery reads GitHub's
`run_attempt` after an uncertain response instead of repeating the POST. An
unresolved write stays pending for the existing looks and incident machinery.
The kernel's existing infrastructure retry bound still applies.

A queue retry waits for Mergify's `dequeued` label or the current head's
`Rule: Queue TL3-approved PRs (queue)` check to conclude. Both check completion
times must be present; an older rule result does not release the wait. A
`dequeued` label left from an earlier queue cycle does not release it either:
GitHub's label events must show it was added after the last queue entry, or
after the dequeue check if no queue entry event is available. When the queue label already
exists, TL3 posts Mergify's documented queue command as its App instead of toggling
the label. The command starts the comment; its effect marker follows it.

The adapter records command dispatch before posting. A restart or uncertain
response cannot send the command twice. Explicit HTTP refusals permit a retry;
unknown outcomes stay pending. Acceptance requires evidence that the PR re-entered
the queue. Missing completion or acceptance uses the existing incident allowance;
a second infrastructure dequeue uses the existing retry bound.
A crash between writing `Sending` and the POST leaves the effect pending until
the usual incident, because recovery cannot distinguish it from a lost response.
A queue-command permission refusal raises an incident naming the App's
**Issues: Read and write** repository permission.

Missing Actions permission raises a setup incident naming **Actions: Read and
write**; see [App setup](../../deploy/github-app/README.md#workflow-rerun-permission).

Run `bash crates/controller/tools/verify-head-checks.sh` through PrismaBuild for the
focused D4 scenarios. The Mergify and publishing suites cover the surrounding flow.
Run `bash crates/controller/tools/verify-classic-protection.sh` through PrismaBuild
for classic branch protection, App bindings, and unavailable protection or status reads.

Workflow recovery waits while a cancelled check's workflow run is still finishing.
A later look reruns the completed workflow before enqueueing. Explicit Actions
refusals retain only a safe reason: missing App access, an expired run, or other
workflow restrictions. A refused POST can be retried with the same delivery key;
a lost response still needs an increased `run_attempt` before enqueueing.

Required head-check failures during the Publishing enqueue step use Q47. Code
failures cancel the pending enqueue and enter the existing fix round. Cancellation
removes `tl3-queue` if present. Infrastructure failures use the same retry bound
as Queued. Check events wake only a pending enqueue on the approved head; the
existing looks recover missed events. Legacy commit statuses satisfy requirements
without an App binding when status reads are available. Classic protection uses
the branch summary and existing Contents read access. No new App permission is
needed. A 403 or 404 on protection or status reads raises one nonblocking setup
incident for each distinct permission gap. Available requirements and queued CI failure
observations are retained; unavailable status-only contexts are skipped. Status
changes rely on existing looks, with no `status` webhook subscription.

Queue observations evaluate the check runs already read with the PR, removing
one duplicate REST read per look. Protection requirements, legacy statuses and
failure annotations stay fresh. A queued look with one App-bound requirement
uses five REST reads instead of six. Cleared setup incidents reopen when fresh
evidence reports that the same permission gap persists.

A required check that never reports raises a missing-check setup incident (Q52).
When a required context from the expected app is absent, the look reads the
head's check suites. If every suite has completed, it opens one nonblocking
incident per head, context and app. A `check_suite` completion on the approved
head now wakes a pending enqueue, like a check run. An enqueue whose required
checks are pending or missing is held with its own `checks_pending` class. That
hold arms no silence timer. A look that finds it releases it to run the guard
again, instead of spending a retry, so a slow or missing check never counts a
failure. Transport-error holds keep the usual allowance. A later report of that
context from that app clears the incident. `clear incident <id>` also clears it,
and it stays cleared for that head. It also clears when its head stops being the
node's head or approved head, or the node closes. Neither path reaches the kernel, so no
failure or credit is charged, and the enqueue still waits for the check. Run
`bash crates/controller/tools/verify-missing-checks.sh` through PrismaBuild for
the focused scenarios.

## Classifier reply checks

`tl3 check-reply classifier` reads one JSON object `{ "inputs": ..., "reply": ... }`
from stdin and writes `{ "violations": [...] }`. It needs no configuration or
store. A valid classification has no violations; an invalid one has the first
repair message, in the worker's original order. Malformed JSON or an unsupported
role exits unsuccessfully. Both this command and controller acceptance call
`classifier::validate`.

The controller gives every worker `TL3_BINARY`, the `tl3` executable beside its
own executable in the immutable release. Test drivers may supply that path
explicitly. The worker's Python code only transports JSON and raises the
returned repair message.

The finite decision proof imports the production `decision.rs` directly through
`src/classifier/kani.rs`. This avoids the kernel's intentionally disabled serde
implementations under `cfg(kani)`. Until #726 provides a Bazel proof target, run
this command inside an admitted PrismaBuild campaign action:

```text
kani crates/controller/src/classifier/kani.rs --harness proofs::classification_total_and_sound --exact --output-format regular -j 1
```

Require `VERIFICATION:- SUCCESSFUL` and all 14 cover checks. A separate disposable
action removes the restartability guard in `decision.rs`; the same proof must
fail its ordered-rule assertion. The unwind bound is 13 for the proof's twelve
rule rows. Production decisions have no loops or collection-size bound.

`//crates/controller:test_test_workers` retains the golden cases, CLI contract,
parse proptests, and real-process repair/tool-denial test. Use
`--test_arg=--classifier-contracts` for the classifier group alone. Python tests
exercise only the shim with a fake executable and orchestration with the Rust
validation boundary stubbed. They never depend on the real `tl3` binary.
