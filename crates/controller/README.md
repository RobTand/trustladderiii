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
keep the returned handle alive. The source sends existing `cli::Input` values.
The daemon's shared ingress loop and production registry remain with #10 and
#20. The default simulation registry still uses the fake.

Before `SubmitChecks`, a maker/worktree caller supplies
`Observation::CheckPlan { node, run: CheckRun { repository, base_sha, plan } }`.
Send this before the maker's completion. `repository` is an absolute local Git
path. `base_sha` is the recorded full commit, not a moving branch. Each check
names its kind and exact pytest node IDs. The controller persists this binding
in `check_plans`, using the existing world-row store. It hydrates the delivery
with that context and the kernel's current head. An unbound plan stays unresolved
with an adapter error. No store migration or kernel rule changes are needed.

`Settings` names a private durable records directory, the trusted installed
`tl3_harness` package directory, the Python command, pinned image, honest CPU and
memory demand, progress allowance and result tool. Use a separate records
directory for each controller store. The Postgres controller claim permits one
adapter writer. The source only reads these records. Retain them across restarts
and upgrades. The records directory also fences older write epochs; reads of
older jobs remain valid. Set `priority` to 0 for served work and -10 for agent
validation. The result tool is `python3` with the published `pbwait.py` argument.
The Python command must provide Python 3.14 on every eligible worker. For the
current fleet, `uv run --python 3.14 --no-project` does that.

The adapter clones committed Git objects into its own clean checkout at the
exact head. Dirty maker files never enter it. A bundle retains both base and
head. The bundle, trusted harness copy, check plan and entry point form a clean
Git input that PB snapshots. Inputs remain unchanged while PB seals them.
`--seal-only` publishes no work. The adapter fsyncs the effect-to-action binding
before `--publish-sealed` can start work. Reassertion uses that saved CAS key,
even after the source checkout disappears or PB publishes a new runtime.
Existing queue records prevent another publication. Retries belong to the
kernel, and every submitted action has max attempts 1.

Queue and bell records decide when to read an ending. A queued or claimed job
stays pending. The adapter uses `pbwait.py --endings --until cleaned --wait-s 0`
to retrieve terminal evidence without waiting for execution. That observation
allowance never ends a job. A withdrawal decision alone cannot confirm cleanup.
Malformed queue records, unavailable results and unverified logs remain unknown.
A valid ending whose verified log has no valid harness report reports
`report: false`. Parsed reports bind the effect, base, head, check names, kinds,
selectors and current `in-process` custody. Only typed fields enter parsing.

The observation includes the action key, base, head and changed paths. The
controller interns those paths in the repository namespace and compares their
IDs with the node's footprint before handing `ChecksDone` to the kernel. Git's
NUL-delimited diff includes both names of a rename by disabling rename detection.
Evidence remains in the store event log and PB's verified result log.

The `prismabuild` controller test executable uses recorded done, failed and
withdrawn queue records from the client crate. Its tool fixture only writes
private queue files. It covers pending work, missing and malformed reports,
wrong heads, transport uncertainty, epoch fencing, withdrawal, SIGKILL followed
by resubmission, source delivery and footprint failures through the real
controller. It submits no fleet work from inside tests.

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
