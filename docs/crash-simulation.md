# Controller crash simulation

The controller commits a kernel step before it dispatches any effect. Its next
transaction records the acknowledgement and queues the result observations.
Recovery reads the world snapshot, delivery table, and upstream records. It
never replays the audit log.

The test owns a controller child, an embedded Postgres server with fsync enabled,
and five persistent upstream fakes. The parent can kill only the controller.
Postgres and the upstream records survive every cut.

## Crash model

`cargo test -p tl3-controller --test crash_safety --locked` runs separate passes
for each save point. In a pass, the parent kills the child once per distinct
operation key, then restarts it from the database. Repeated visits to that key
continue normally. Delivery transitions use the effect key and source and target
states, because recovery can revisit a transition at a new audit revision.
Each pass ends at the same world as an uninterrupted run.
A final coverage check requires every committed operation in that pass to
appear in the injected cuts.
Separate passes let an earlier cut leave later save points reachable.

Every cut uses SIGKILL. A panic or dropping the controller would run destructors
and would not establish process-loss equivalence. Full independent prefix
replays were considered, but repeat all completed work for every cut. The chosen
passes retain progress and test repeated crashes during recovery itself.

Before killing the child, the parent reads all world rows, versions, arrival
sequences, pending and terminal effects, ledger entries, interned values, and
audit events. After Postgres releases the dead session's advisory lock, these
must match exactly. Interior cuts also require Postgres to report a live idle
transaction before the kill. There is no delay-based death or recovery test.

The terminal comparison includes the entire kernel world, effect kinds and
terminal states, credit amounts, upstream resources, and epoch. It normalizes
revision-derived message keys and intern IDs. PostgreSQL sequences can have
gaps after an aborted insert. Audit history and row revisions also grow during
recovery. Those are expected differences, not missing work.

## Save points

`Store::set_boundary_hook` covers controller writes, including initialization,
intake, observation arrival and consumption, delivery transitions, acknowledgements,
cancelled absence, watch results, and epoch upgrades.

| Point | Cut |
|---|---|
| `EventWritten` | Audit insert is uncommitted |
| `RowsWritten` | World and inbox writes are uncommitted |
| `EffectsWritten` | New outbox rows are uncommitted |
| `TransitionsWritten` | Delivery changes are uncommitted |
| `CreditWritten` | Ledger changes are uncommitted |
| `BeforeCommit` | All writes and metadata are uncommitted |
| `AfterCommit` | Commit succeeded, caller has no receipt yet |
| `InternBeforeCommit` | Dictionary insert is uncommitted |
| `InternAfterCommit` | Dictionary insert committed before its caller receives the ID |
| `BeforeDispatch` | Sent committed, upstream has not been called |
| `AfterDispatch` | Upstream returned, acknowledgement is not committed |
| `BeforeReconcile` | Upstream record has not been read |
| `AfterReconcile` | Read or duplicate deletion happened, result is not committed |

Schema migrations and read-only transactions are outside this controller
save-point matrix. The existing store suite covers migration behavior.

## Scenarios and replay

`TL3_SIM_SEED` selects deterministic lost responses and the fake clock's initial
time. Failure output prints the seed. `TL3_SIM_SCENARIO` and `TL3_SIM_POINT`
restrict a replay. A replay must still execute at least one crash.

Scenarios cover a merge, a clean rebase followed by fresh checks and review,
stopping a run, stopping checks, stopping a queued PR, cancelling before
sending, cancelling a sent attempt that exists or is absent, retryable and
permanent failure, and an upgrade with effects in flight.

The existing fake adapters supply typed results. The simulation adds upstream
records that enforce once-per-key starts, once-per-head approvals, converging
resource writes, and marked messages. Messages simulate a lost response and two copies.
Reconciliation deletes the extra copy. Each non-cancelled delivery must have
an upstream record, and each successful logical application must occur once.
Acknowledged asynchronous starts return their results through watches. Some
initial reads report Pending before their authoritative terminal result.

## Clock and ingress

`clock::Clock::now() -> SystemTime` is the wall-clock seam for liveness timestamps
and ticket #15's allowance probes. `Controller::open` uses `SystemClock`.
`Controller::open_with_clock` accepts a fake. `Controller::now` reads the selected
clock. Advancing it never proves failure or absence. The simulation advances it
by days while awaiting upstream evidence.

Tickets #9 and #10 are absent from this branch. `ingress_loom.rs` models the
wave brief's two source threads and one controller owner over a channel. It
checks publication of durable webhook evidence, successful worker exit before
its reply, source order, and one-time consumption. A standard-channel test
checks send errors after the owner has gone, because loom 0.7 does not model
disconnection. Integration must replace these model sources with the real
ingress and worker-output source when those branches land.

## Fleet verification

`crates/controller/tools/crash_manifest.py` generates five PrismaBuild actions.
Three require the host tags `dl380g10`, `sparky`, and `sparklina`. Two allow any
worker, with separate seeds so they cannot reuse one action's receipt. All five
run the controller scenarios and ingress tests. One also runs workspace
formatting, Clippy, all Rust tests, and the kernel build without default features.
The simulation prints its own elapsed time and number of real process kills.

The crash matrix uses a small seeded footprint. The tracer's existing 80-file
case remains in the controller suite and runs in every placement too.
