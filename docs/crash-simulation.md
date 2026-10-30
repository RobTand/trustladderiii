# Controller crash simulation

The controller commits a kernel step before it dispatches any effect. Its next
transaction records the acknowledgement and queues the result observations.
Recovery reads the world snapshot, delivery table, and upstream records. It
never replays the audit log.

The test owns a controller child, an embedded Postgres server with fsync enabled,
and five persistent upstream fakes. The parent can kill only the controller.
Postgres and the upstream records survive every cut.

## Crash model

`cargo test -p tl3-controller --test crash_safety --locked` defaults to the
`merge` smoke scenario, with separate passes for all thirteen save points.
Set `TL3_SIM_FULL=1` to run all eleven scenarios and require all sixteen
effect-kind and emitting-phase pairs. The smoke run skips that matrix-wide
coverage assertion. Explicit scenario and save-point filters still restrict a replay.

In a pass, the parent kills the child once per distinct operation key, then restarts it from the database. Repeated visits to that key
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
an upstream record. A fake counts every execute of an already recorded key,
even when it returns the cached completion. The recovered run requires zero
such redeliveries because the controller reconciles existing records first.
Each node's ledger sum must also equal its kernel credit balance, independently
of the uninterrupted baseline.
Acknowledged asynchronous starts return their results through watches. Some
initial reads report Pending before their authoritative terminal result.

## Clock and ingress

`clock::Clock::now() -> SystemTime` is the wall-clock seam for ticket #15's
liveness timestamps and allowance probes. `Controller::open` uses `SystemClock`.
`Controller::open_with_clock` accepts a fake. `Controller::now` reads the selected
clock. Only tests call `Controller::now` today. The controller loop does not
read the clock yet; #15 will be its first user. Advancing it never proves failure
or absence. The simulation advances it by days while awaiting upstream evidence.

Epoch refusal is checked fake-against-fake: the contract check writes a newer
epoch, reopens the fake, and attempts an older write. The controller loop never
reaches that older-epoch dispatch. The store re-epochs Intended effects during
an upgrade and refuses upgrades with Sent, Uncertain, or Cancelling deliveries.

`ingress_loom.rs` is a placeholder. Its two model sources exchange `cli::Input`
values over a loom channel, but it runs no controller, ingress, or worker source
code. The checks describe the intended publication and source-order rules only.
A standard-channel test checks disconnection because loom 0.7 does not model it.
Ticket #71 will model the real ingress and worker source after #64 and #60 merge.
The current loom model does not cover the controller's concurrent code.

## Fleet verification

`crates/controller/tools/crash_manifest.py` generates five PrismaBuild actions.
Three run the default smoke scenario with the host tags `dl380g10`, `sparky`,
and `sparklina`. One portable action sets `TL3_SIM_FULL=1` for the full matrix.
The other runs `verify.sh` with a separate seed and the smoke default, including
workspace formatting, Clippy, all Rust tests, and the kernel build without
default features. The host and workspace actions also run the existing
controller scenarios, worker process scenarios, and placeholder ingress tests.
The portable full action runs only `crash_safety`.
The simulation prints its own elapsed time and number of real process kills.

The crash matrix uses a small seeded footprint. The tracer's existing 80-file
case remains in the controller suite and runs in every placement too.
