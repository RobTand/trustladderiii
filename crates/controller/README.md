# TL3 controller

`tl3-controller` connects the pure kernel to the Postgres store and upstream
executors. Read [the architecture note](../../docs/architecture.md) for the
transaction and recovery contracts.

All commands use the same TOML configuration as `tl3-config`. Controller execution
supports explicit simulation only. Status commands read the configured store
and do not load GitHub or model credentials.

The daemon uses `Config::load`, which checks all secret-file metadata. Status
uses `Config::load_status`, which checks only the optional store password.
Only the store reads its password at runtime.
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

`adapters::simulation_with_workers` replaces ingest, audit, planning and maker with Python
worker processes and returns a `WorkerOutputSource`. Review remains on
the worker fake. Supply issue and repository text through `Store::put_worker_materials`
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

Production integration still requires:

1. The controller's production material assembler must write the `maker` object
   with `Store::put_worker_materials` before dispatch. There is no production
   caller today. Live tracer #20 must connect the accepted brief, stable goal IDs,
   and planner check plan to this store API. The supervisor supplies kernel fields
   and deployment settings; it does not invent a brief or checks.
2. Mergify #63, PR #69, has merged. Connecting its queue evidence to maker
   feedback with the failed check's name and link or summary remains pending.
   Its current bare fact is supported but lacks details.

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
Three rejected comment clarifications post a fixed, checked request to rephrase.
Other rejected notices fail permanently and post nothing. Writer transport errors
leave the delivery unconfirmed.
The reply uses the original effect marker, so recovery does not restart the
writer loop once the reply exists.
`FakeWriter` returns fixed prose in simulation. The seam uses the existing writer
role's protocol v1 input and reply shapes. Questions retain the kernel's allowed
options. Closing summaries close the issue, except when the request is replaced.
PR effects still use this module's fake until ticket #13. Incident notices stay
on the fake until the operations issue is wired in. Replacements start a new
request on the same issue from Rob's text. Corrections create linked issues
with the `tl3` label.
The checker uses the request's own `Delivery.paths` and any further paths
supplied in the adapter policy. No build script inventories TL3's checkout.
It rejects both `:line` and `:line:column` suffixes.

The `github` integration target uses the recorded webhook fixtures, the actual
HTTP receiver, a local REST server and embedded Postgres. It covers delivery
persistence, unauthorized labels, gap recovery, duplicate reconciliation, writer
rejections, epoch fencing and SIGKILL after posting before acknowledgement.

`tl3d --config FILE --simulate DIRECTORY --workers` registers the worker output
source alongside stdin and optional webhooks. It uses configured Python for
ingest, audit, planning and maker, with the remaining roles and upstreams simulated. Add
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
The kernel has no dequeue effect, so this adapter adds none. GitHub PR effects
remain on the fake until ticket #13.

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

The following handoff describes the state before PR #80.

Ticket #15 owns allowances and scheduled looks. On an unchanged authoritative
read after the progress allowance, it can report: "GitHub merged at X, Mergify
check Y still in_progress", using `merged_at`, `queue_check.id` and
`queue_check.status`. The read itself sets no timer and emits no stall incident.
The kernel currently has no Mergify-stall fact. Ticket #15 must supply that path;
this adapter does not change kernel rules.

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
The production registry remains with #20. The default simulation registry still
uses the fake.

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
closes and every effect and watch for that node has settled, ticket #13 teardown
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

`plans` holds the latest schema-valid planner reply, accepted or rejected;
`plan_reports` retains each generation's evidence. Malformed output fails the run
and does not replace either document.
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
item's size using Postgres's JSONB text representation. A record that cannot be
queued is logged and dropped so memory cannot block worker retirement.

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
"Hindsight rejected TL3's key". Other 4xx responses park the row with its status.
Transport failures and 5xx responses leave rows pending for later wakes. The
active retry set holds at most 10,000 rows; new records beyond that limit park
the oldest pending rows. Parked payloads remain available for operations review
and need archive retention management. The status page shows pending and parked
counts and any key-rejection incident.

Records have `repo:`, `request:` and `role:` tags. The controller selects summary
fields, omits infrastructure and repository file contents, removes credential
fields, and redacts bearer tokens, common provider keys, private keys,
credential assignments and passwords in URLs before enqueueing. Each item is
at most 32 KiB. Oversized sections carry a truncation marker. Pending reads use
64-row pages, and acknowledged items are deleted without a local receipt
history. Public action IDs are preserved in structured test activity, while
generic input fields ending in `_key`, `_token` or `_secret` are removed.
Full transcripts remain the tracing system's responsibility.

`cargo test -p tl3-controller --test hindsight --locked` uses only a fake server.
It covers every discovered role, outages, lost acknowledgements, real SIGKILL
cuts, database restart, secret filtering, bounded records and cleanup. The
worker scenarios also prove that requests close during a memory outage and
leave their completed run records queued. Python's `test_memory.py` covers the
shared runtime collector for every role.
