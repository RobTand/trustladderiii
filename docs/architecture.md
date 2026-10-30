# Controller architecture

`tl3-kernel` makes decisions. `tl3-controller` loads its world, applies one event,
commits the result, and routes the outputs. Upstream client crates do not import
the controller or the kernel. All Rust crates share the root workspace and lockfile.

## Persistence

The store owns the SQL schema. `tl3_events` is an append-only audit log.
`tl3_world` holds named JSON rows with their last changed revision.
`tl3_effects` holds immutable keys, epochs and delivery states. `tl3_credit`
records balance changes once per request and event. `tl3_intern` maps external
values to stable IDs. The store commits these together with a revision check.

The controller partitions a serialized kernel world into `nodes`, `gates`,
`file_sets` and a `kernel/meta` row. It writes only changed rows. Closed nodes
and file sets retain their indices. `repos` binds dense kernel gate indices to
configured repositories. Store repository IDs can contain gaps and never index
gates. `requests` binds node indices to issue numbers. `pull_requests` records
PR numbers. `inbox` holds observations awaiting a kernel step. `watches` retains
acknowledged operations whose upstream result may arrive later.

The kernel's optional `serde` feature supplies the snapshot encoding without
adding I/O. Separate controller DTOs were considered, but would duplicate every
kernel field and variant. Recovery validates snapshot format 1 and the kernel
invariant. A schema change needs an explicit migration and epoch change.
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
while its owner is still sending it.

After a crash, Sent becomes Uncertain. Reconciliation reads the upstream's
record by key. A found result confirms the effect and durably queues its replies
or facts. Authoritative absence returns the effect to Intended. A pending or
unknown attempt stays pending. Acknowledged starts also retain a watch, so a
missed worker reply does not require redispatching its start. No duration or
silence alone declares work lost.

The kernel assigns a new sequence after a retryable acknowledgement. The
controller retires the previous delivery through Intended to Cancelled in the
same transaction, so it cannot also resend the superseded effect.

## Adapters

`executor::Executor` exposes `execute` and `reconcile`. `Delivery` carries the
key, epoch, kernel output, operation generation, repository, issue, PR, exact
SHA, paths, attempt and round. The PR binding is refreshed before dispatch.
`Completion` carries an acknowledgement and observations. An asynchronous start
can acknowledge immediately and publish its reply later through `observe`.
Reconciliation returns Found, Absent or Pending. Absent promises that a prior
attempt has ended. Transport errors carry no such promise.

Each upstream has a module under `controller/src/adapters`. To replace one fake,
implement `Executor` in that module using the existing client crate and replace
one `register` line. The exhaustive router owns the mapping of effect kinds to
upstreams. Adapters fence epochs at their upstream boundary. They reconcile
uncertain writes before retrying them. They report evidence and never choose
request phases, credit charges or retry policy.

The worker fake returns typed observations at the supervisor boundary. A real
worker adapter must use protocol v1 from `workers/README.md`, validate its role
schemas, and accept a reply only after a successful process exit. Worker
progress is liveness data, not a kernel verdict.

Simulation fakes persist receipts and delivery counts in a separate directory.
They survive controller process death. They represent upstream records, not a
replacement for Postgres. They cover every effect kind, including rebase and
teardown. A fake queue receipt supplies a merge fact independently of its enqueue
acknowledgement. Production GitHub ingress, process supervision and live clients
remain the following tickets.

## Commands and verification

`tl3 status --config FILE` reads Postgres and prints request, phase, wait and
credit. It does not need worker or GitHub credential files.

`tl3d --config FILE --simulate DIRECTORY` reads one JSON `Input` per stdin line.
It recovers existing work before accepting input, drains available events after
each line, and exits at EOF. `--drain` performs recovery without reading stdin.
Simulation is explicit. This binary makes no live upstream calls.

Submit `bash crates/controller/tools/verify.sh` through PrismaBuild. It checks
formatting and Clippy, tests every workspace crate, and builds the kernel without
default features. The controller tests share the store's embedded Postgres fixture
with fsync enabled. They drive an issue to `Closed(Merged)`, run the actual status
binary, kill controller children with SIGKILL at durable boundaries, and check
that every recorded effect was delivered once. Extra cases cover Sent with no
upstream record, a lost reply after acknowledgement, and kernel-directed retries.
