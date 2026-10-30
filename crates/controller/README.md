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
`adapters::simulation` is currently the only registry builder. A test starts
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

For the review-fix evidence, create a clean detached worktree at the final
commit, then generate the campaign there:

```bash
python3 crates/controller/tools/review_manifest.py > /tmp/tl3-review-campaign.json
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbcampaign.py --detach /tmp/tl3-review-campaign.json
```

The campaign adds the kernel tests and three Kani harnesses covering retryable
acknowledgements, publishing transitions, and queued progress. Read each terminal
record, log, and CAS receipt before reporting a pass.

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

Prepare pins the fetched main commit before creating branch, base and checkout.
Before confirming an unfinished Prepare, it verifies the checkout matches its head.
A partial checkout is repopulated only if its head is still the recorded base.
No maker has run at that point. Completed Prepare records never repair maker files.
Push pins the delivery's exact head and leases against the last head this adapter
published. Rebase pins its target before execution, so recovery cannot silently
use a newer main. A clean rebase reports `Observation::Rebased`; the controller
interns its SHA before the kernel receives `RebaseClean`. A conflict reports
`RebaseNeedsChanges` after aborting and verifying a clean checkout at the old
head. The kernel alone selects the next phase.

Teardown records that the checkout is clean before deletion begins. A retry may
finish removal if its only changes are deletions. Edits and new files are refused.
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
