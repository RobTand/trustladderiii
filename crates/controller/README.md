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
`adapters::simulation` is currently the only registry builder. A test starts
with it and replaces one entry with `register`. The `mixed_registry` scenario
registers `worktrees::Real` for all four worktree effects against local bare
repositories over `file://`. The other four upstreams stay fake. The worker
fixture reports the actual local head so real pushes can use it.

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

`worktrees::Real::new(&config.worktrees_root)` uses the existing configurable
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
and worktree root, records the highest accepted epoch, and journals immutable
operation intents and terminal evidence. Changing that root requires an explicit
migration of existing checkouts and records. Records use atomic replacement and
sync both the file and its directory. No store migration is required.

Prepare pins the fetched main commit before creating branch, base and checkout.
Push pins the delivery's exact head and leases against the last head this adapter
published. Rebase pins its target before execution, so recovery cannot silently
use a newer main. A clean rebase reports `Observation::Rebased`; the controller
interns its SHA before the kernel receives `RebaseClean`. A conflict reports
`RebaseNeedsChanges` after aborting and verifying a clean checkout at the old
head. The kernel alone selects the next phase.

Teardown removes the clean checkout, leases deletion of the recorded remote
head, then removes the local branch and base. A dirty checkout or changed remote
ref returns an adapter error. Unknown outcomes keep their original effect key.
Interrupted current-epoch operations reconcile against Git and finish their
original intent. Older epochs may read terminal evidence, but cannot resume
unfinished Git writes. No duration or silence establishes absence.

The `worktrees` test executable kills adapter children with SIGKILL before and
after Git applies each effect, including conflict and stale-epoch cases. The
workspace verification script runs it, the mixed registry lifecycle, and the
existing controller and store scenarios through PrismaBuild.
