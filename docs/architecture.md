# Controller architecture

`tl3-kernel` makes decisions. `tl3-controller` loads its world, applies one event,
commits the result, and routes the outputs. Upstream client crates do not import
the controller or the kernel. All Rust crates share the root workspace and lockfile.

## Persistence

The store owns the SQL schema. `tl3_events` is an append-only audit log.
`tl3_world` holds named JSON rows with their last changed revision and a
monotonic insertion sequence. Inbox observations are consumed in that arrival
order, including observations written in one transaction. Updating a row keeps
its insertion sequence. Keys never decide event order.
`tl3_effects` holds immutable keys, epochs and delivery states. `tl3_credit`
records balance changes once per request and event. `tl3_intern` maps external
values to stable IDs. The store commits these together with a revision check.

The controller partitions a serialized kernel world into `nodes`, `gates`,
`file_sets` and a `kernel/meta` row. It writes only changed rows. Closed nodes
and file sets retain their indices. `repos` binds dense kernel gate indices to
configured repositories. Store repository IDs can contain gaps and never index
gates. `requests` binds node indices to issue numbers and optional titles. `pull_requests` records
PR numbers. `inbox` holds observations awaiting a kernel step. `watches` retains
acknowledged operations whose upstream result may arrive later.

The kernel's optional `serde` feature supplies the snapshot encoding without
adding I/O. Separate controller DTOs were considered, but would duplicate every
kernel field and variant. Recovery validates snapshot format 1, sorted and
deduplicated footprint rows, and the kernel invariant. It rejects malformed footprint rows before any lock
lookup can use binary search. A kernel snapshot change needs an explicit
migration and epoch change.
Store migration 2 adds arrival sequences. It backfills existing rows by revision,
table and key. Drain the old inbox before migrating, because its ordering within
one old transaction was never stored.
Recovery never replays the audit log. This ticket adds no upgrade command.

SHA IDs are scoped to a node. Path IDs are scoped to a repository. Issues and
PRs use separate namespaces. The controller interns real values before handing
them to the kernel. Large footprints retain their ordered world-local table.

## Event loop and recovery

1. Load a consistent store snapshot.
2. Translate one observation, then call `step`.
3. Commit its event, changed world rows, credit changes and Intended effects.
4. Mark one effect Sent in a separate durable transaction.
5. Execute it with its key and current epoch.
6. Commit its acknowledgement, effect transition and result observations together.
7. Consume each observation as another event.

An effect key uses the stable node index and kernel sequence. Notices use the
creating revision and output index. Duplicate intake deliveries return the
original transaction receipt. Separate label deliveries for one issue still
create one request. Intake requires the configured actor, repository,
installation and `tl3` label.

The controller holds a session advisory lock in Postgres for its lifetime.
Status readers do not take it. A second daemon cannot reconcile an attempt
while its owner is still sending it. Postgres may still hold the dead session
briefly after SIGKILL. `Controller::open` fails clearly when ownership remains
held and lets the service manager restart it after that session ends. It does
not infer death from elapsed time or steal the advisory lock. Crash tests wait
on the database releasing that lock, rather than sleeping before restart.

After a crash, Sent becomes Uncertain. Reconciliation reads the upstream's
record by key. A found result records the upstream acknowledgement and durably
queues its confirmed replies or facts. A found retryable or permanent failure reaches the
kernel as that acknowledgement, through the permitted delivery transitions.
Authoritative absence returns the effect to Intended. A pending or unknown
attempt stays pending. Acknowledged starts also retain a watch, so a
missed worker reply does not require redispatching its start. No duration or
silence alone declares work lost.

An `execute` error means the response is unknown. The effect remains Sent and
then becomes Uncertain. A reconciliation error or Pending result changes no
acknowledgement or kernel state. Errors are isolated to their delivery, so other
requests continue. The controller never converts a lost response into Retryable:
the kernel would issue a new key and could duplicate the outside write.

The tracer reconciles unresolved effects and watches when `drive_once` runs,
including after restart. Ticket #15 owns the looks that wake it again when no
other input arrives. Clocks and silence prove neither failure nor absence.

The kernel exposes `Node::waiting_on`, `operation_pending` and
`effect_cancelling` as pure read queries. Status formats the first. The controller
uses the others to retain watches and settle deliveries after a node moves on.
The query module changes no kernel transition, `step`, or `settle`.

An old-epoch Intended effect becomes Cancelled without dispatch. An old-epoch
Sent or Uncertain effect becomes Cancelling and is reconciled by its original
key and epoch. A moved-on node's cancelled delivery uses the same paths.
Authoritative absence makes it Cancelled and reports Retryable to the kernel,
never a fabricated confirmation. The kernel ignores that acknowledgement when
its in-flight effect is cancelling. A found cancelling attempt becomes final
Confirmed in the delivery table, passes through its actual acknowledgement and
observations, and leaves cleanup to the kernel.

Normal upgrades still follow the state table: drain Sent, Uncertain and
Cancelling effects first, migrate a snapshot, then bump the epoch. The store
refuses a premature bump. Intended effects retain their keys and receive the
new epoch. Defensive stale-effect recovery does not bypass that upgrade rule.

The kernel assigns a new sequence after a retryable acknowledgement. The
controller retires the previous delivery through Intended to Cancelled in the
same transaction, so it cannot also resend the superseded effect.

## Adapters

`executor::Executor` exposes `execute` and `reconcile`. `Delivery` carries the
key, epoch, kernel output, operation generation, repository, issue, PR, exact
SHA, paths, attempt and round. The PR binding is refreshed before dispatch.
`Completion` carries an acknowledgement and observations. An asynchronous start
can acknowledge immediately. Late replies and webhooks enter through `tl3d`'s
ingress. Source threads send `cli::Input` values on one
`std::sync::mpsc::Sender<Input>` to the thread that owns the `Controller`.
That thread applies intake or calls `observe`, then drains `drive_once`.
`observe` stays the controller entry point for those observations. Adapters do
not reach the controller. Ticket #10 adds `ingress.rs` and this loop. The tracer
keeps the stdin loop until that ticket lands.
Reconciliation returns Found, Absent or Pending. Absent promises that a prior
attempt has ended. Transport errors carry no such promise.

Each upstream has a module under `controller/src/adapters`. To replace one fake,
implement `Executor` in that module using the existing client crate and replace
one `register` line. The exhaustive router owns the mapping of effect kinds to
upstreams. Adapters fence writes at their upstream boundary against a durable
highest accepted epoch, rather than comparing two rows in the controller's
store. Reads may still reconcile older records by their original key. Adapters
reconcile uncertain writes before retrying them. `Error::Adapter` can wrap a real client's
error. Only an upstream answer belongs in `Completion::ack`. A transport error
belongs in `Err`, and a reconciliation transport problem can also stay Pending.
Adapters report evidence and never choose request phases, credit charges or
retry policy.

The worker fake returns typed observations at the supervisor boundary. A real
worker adapter must use protocol v1 from `workers/README.md`, validate its role
schemas, and accept a reply only after a successful process exit. Worker
progress is liveness data, not a kernel verdict.

Each adapter's ticket extends `Delivery` with the fields it needs, such as
issue text in #10 and branch and PR text in #13. Those fields are not added
speculatively here.

`adapters::simulation` is currently the only registry builder, and `tl3d`
requires `--simulate`. A test can replace one entry while keeping the other fakes:

```rust
let mut registry = adapters::simulation(fake_records)?;
registry.register(Upstream::GitHub, github_adapter_using_a_local_mock);
```

The `mixed_registry` controller scenario does this with the real worktree
client in a temporary Git repository. Its test adapter delegates worktree
operations outside that test's scope to the worktree fake. The same registration
shape lets #10 test its real GitHub adapter against local fixtures.

Simulation fakes persist receipts, delivery counts and every accepted delivery's
epoch in a separate directory. They persist a highest accepted epoch and refuse
older writes after an adapter restart. Reconciliation can read earlier epochs.
They survive controller process death. They represent upstream records, not a
replacement for Postgres. They cover every effect kind, including rebase and
teardown. A fake queue receipt supplies a merge fact independently of its enqueue
acknowledgement. Production GitHub ingress, process supervision and live clients
remain the following tickets.

## Commands and verification

`tl3 status --config FILE` and `tl3 status-serve --config FILE` use one status
model. They show issue titles and numbers, phase, wait, credit, last progress
and open incidents. The page serves GET and HEAD without login at the configured
listen address, in a separate process from `tl3d`. Both commands use a reader
that skips migrations and enforces read-only, repeatable-read transactions.
They do not need worker or GitHub credential files. Tickets #9 and #15 fill
`status::load_request_details` when their store tables exist. It receives all
node IDs together and returns a map of public progress and incident summaries,
using batch queries in the same read-only transaction.

Both commands use `Config::load` to check secret-file metadata, including the
optional store password. They do not read GitHub or model secrets in simulation.
`limits.agent_pool` configures bootstrap capacity and defaults to 1. Existing
world capacity requires a snapshot migration.

`tl3d --config FILE --simulate DIRECTORY` reads one JSON `Input` per stdin line.
It recovers existing work before accepting input, drains available events after
each line, and exits at EOF. `--drain` performs recovery without reading stdin.
Simulation is explicit. This binary makes no live upstream calls.

Submit `bash crates/controller/tools/verify.sh` through PrismaBuild. It checks
formatting and Clippy, tests every workspace crate, and builds the kernel without
default features. The controller tests share the store's embedded Postgres fixture
with fsync enabled. They drive an issue to `Closed(Merged)`, run the actual status
binary, and kill controller children with SIGKILL after intake, pickup,
dispatch, the 80-file plan, the PR binding, and enqueue. Before each kill the
parent snapshots the parked child's store. Restart must preserve every row,
version, insertion sequence, effect, credit entry and interned value. Recovery
may change only the selected Sent delivery to Uncertain and its revision.
The fake delivery-count keys must equal all non-Cancelled effect keys, with
each delivered once, so missing deliveries also fail.

Other cases cover adapter outages with another request progressing, a lost
response retaining its original key, worker and check and rebase and enqueue
watches, Pending reconciliation, found non-confirming acknowledgements,
cancelled absence, stale epochs, upgrade drain guards, malformed footprints,
read-only status and configuration permissions. The review campaign generator
in `crates/controller/tools/review_manifest.py` adds kernel tests and three
representative Kani proofs. Generate it from a clean detached worktree at the
final commit. CI runs independently on Mergify runners.

The PrismaBuild checks adapter is available for mixed registries. A
`CheckPlan` observation records the base commit, Git repository and named checks
in a `check_plans` world row. Maker/worktree integration supplies it before the
maker's completion. `Delivery::checks` carries that context and `job_generation`
identifies the job to withdraw after the node changes generation.

The adapter freezes a clean head checkout, bundles both commits and snapshots
that bundle with a trusted harness copy. It seals without publication, fsyncs
the effect's action key in its private durable directory, then publishes that
exact CAS request. That directory is scoped to one store and its single
controller owner. A durable epoch fence rejects old writes. Results still
reconcile by their original key and epoch. Existing PB records prevent repeated
publication, including failed and withdrawn actions. PB owns stalls; the
adapter sets a tests progress contract and max attempts 1, with no duration
limit.

`prismabuild::Source` runs on its own thread with a `Sender<Input>`. It watches
only the adapter's saved action keys, rescans on bell reset and retries failed
per-key reads. It verifies cleaned endings and report-log digests before
sending checks evidence. Sources never touch the controller. The common daemon
loop remains ticket #10's work; this ticket supplies the one-line registration
`adapter.source().spawn(sender)` for that loop. The controller translates the
changed-file evidence through repository-scoped path interning and supplies
`in_footprint` to the kernel. No kernel transition or store schema changed.
