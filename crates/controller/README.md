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
Until #61 merges, the maker acceptance fixture creates that git worktree itself.
Attempt N selects `models.maker[N - 1]` for both rounds. The kernel alone
advances attempts. Each attempt has a conversation outside the worktree under
`workers.runs/sessions/NODE/plan-REPLANNED/attempt-N.json`. Round 1 resumes
it. A fitting `needs_files` resumes at round 0 within the same attempt and plan.
The resume decision checks the stored reply against the delivery attempt and
re-plan flag, so a restart at attempt 1 after a re-plan starts fresh.
A separate round checkpoint records the starting commit before the model runs.
A retry counts commits since that checkpoint, including a commit left by a crash.
Only clean completions save conversation history.

After durable work drains, the supervisor deletes a closed node's session
directory only when no pending effect or retained watch can reconcile that node.
Open requests keep their sessions. The directory itself survives a crash as
the cleanup queue, and the next drain repeats deletion. No clock triggers cleanup.
Worker code commits a completed change and computes a conservative fit bound
for additional files. The controller interns returned heads and requested paths
before delivering kernel replies. It stores the latest reply as `maker_replies`.

Maker acceptance runs the actual Python entry point and maker graph with a
scripted model, fake read-only Hindsight, and recorded self-check results.
The worktree and PrismaBuild adapters stay fake until #61 and #62 merge.
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
2. Mergify #63, PR #69, must emit `Observation::CiFailed` with the failed check's
   name and link or summary. Its current bare fact is supported but lacks details.
3. PrismaBuild #62, PR #67, must map its parsed harness report into `failed_tests`
   and the finding report, and compare `changed_paths` with the footprint to fill
   `outside_paths`. The current fake reports only verdict booleans; it cannot
   supply missing test messages or offending paths.
4. Replace the acceptance worktree setup with #61's real adapter, PR #68, and the
   check fake with #62's real adapter and result source, PR #67. Keep recorded
   terminal records and CAS receipts in tests. Then run a real maker change
   through the real harness on PrismaBuild and record one live trial from the
   implementation session. Admitted tests consume records and never submit a
   second PrismaBuild action.

Generate all-host verification with
`python3 crates/controller/tools/worker_review_manifest.py maker`, then submit
the manifest with `pbcampaign.py --detach`. The three pinned actions run maker, worker, supervisor and GitHub ingress scenarios.
The daemon fixture makes and commits a real change through the shared ingress loop.
A separate portable action runs the Python worker and harness suites and
`tools/verify.sh`. These tests use fixtures and never submit nested jobs.

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
installation and comment id. Ticket #14 classifies them.

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
PR effects still use this module's fake until ticket #13. Incident and replacement
notices also stay on the fake for their owning tickets. Incidents need the
operations issue, and replacements need a new request, not another comment.
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
