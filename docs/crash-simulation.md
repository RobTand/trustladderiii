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
`merge` smoke scenario, with separate passes for all twenty save points.
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
intake, ingress receipt and consumption, catch-up, observation arrival and consumption, delivery transitions, acknowledgements,
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
| `ReceiveIngressBeforeCommit` | Webhook receipt is uncommitted |
| `ReceiveIngressAfterCommit` | Receipt is durable before its caller gets a result |
| `ConsumeIngressBeforeCommit` | Consumption, intake cursor, and retirement are uncommitted |
| `ConsumeIngressAfterCommit` | Consumption and retirement are durable |
| `ReceiveCatchUpBeforeCommit` | Catch-up deliveries and cursor are uncommitted |
| `ReceiveCatchUpAfterCommit` | Catch-up deliveries and cursor are durable |
| `IngressApplied` | Translated inputs committed, delivery not yet consumed |

Translator registration and translator cursors, schema migrations, and read-only transactions are outside this controller
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

`crates/controller/tools/verify-loom.sh` enables `cfg(loom)` only in the
controller crate. Setting `RUSTFLAGS=--cfg loom` for every dependency would also
replace Tokio's runtime internals, including the runtime used by Postgres and
HTTP. The optional `loom` Cargo feature enables the controller's shims through its
build script. Default production builds do not include the Loom dependency.

The models call real code:

- `Sources::register` and `Sources::run` apply inputs to a real controller and
  Postgres store. Two sources interleave distinct deliveries and repeated issue
  labels. Acknowledgements from committed intake events end the finite model.
- Two durable webhook sources call the production request handler, including
  `receive_ingress`, then wake `Sources::run`. The model checks application
  before consumption, one event per delivery, and an empty pending queue.
- `WebhookControl::gap` and `stop` preserve queue order. An HTTP model also calls
  `Control::stop` and `accept_connections` with accept errors and a simulated
  wake. It checks that stop precedes the wake and publishes earlier state through
  release/acquire. The model opens no sockets.
- `WorkerOutputSource::poll_once` reads actual partial JSONL output, persists
  progress and declared waits, and emits ingress inputs. A concurrent consumer
  checks persistence before publication. Restart rereads the same output and
  must emit no repeated progress.
- The PrismaBuild source races durable wait registration with its node snapshot,
  consumes the registration notification, and calls its real `poll` and client
  watcher against a local HTTP bell and recorded queue files. Separate models
  cover a reset and HTTP 503. Each checks that a live record emits no look, an
  ending emits one look per batch, and removal leaves no awaited wait. The live
  queue record exists before registration, since `poll` may select the watcher
  while registration notifications remain queued.

The controller models exhaust a preemption bound of zero. They still explore
blocking source interleavings. The earlier controller model took about 34 minutes
at bound one on dl380g10. Set `TL3_LOOM_NIGHTLY=1` when submitting
`verify-loom.sh` through PrismaBuild to opt into a nightly bound-one controller
lane. The default lane stays at bound zero. `TL3_LOOM_BOUND=N` overrides the bound
for the shared model runner, including focused bell reruns. The worker, bell,
and webhook control models normally use bound one. HTTP stop uses bound two. The shared runner has
a 10,000-branch limit that fails rather than skips an unfinished execution. The model body and I/O actors
use 8 MiB stacks because Loom's default coroutine stack cannot run these library
calls. Channels and shared source locks use Loom primitives. Postgres, inotify,
HTTP, Tokio scheduling, and socket deadlines remain external operations. An eighth
model checks that the channel shim rejects sends after receiver drop. Loom 0.7
does not model the timed-look branch: Loom 0.7.2's `recv_timeout` is
`unimplemented!`. The models keep the controller out of that branch. Loom also
does not model sender disconnection, so the finite controller models end through
an explicitly acknowledged source error. Ordinary channel
tests cover standard-library disconnection. The
worker fixture serializes file append steps against individual source passes;
it includes an incomplete line and checks every resulting input. These models
check source publication and registration schedules, not every possible OS I/O
interleaving. The process suites and SIGKILL simulation cover process loss.

`loom_mutants.py MUTANT` changes one production site in a scratch checkout,
prints its diff, runs the relevant model, and requires the expected assertion
or Loom deadlock. It restores the source afterward. `--diff` only prints the
patch. Mutants cover input loss, wake before durable receipt, consumption before
application, missing owner drain, a lost gap, stop loss and reordering, relaxed
stop atomics, progress replay, progress without persistence, wait registration,
the awaited key, and reset or outage rereads.
Run mutations through PrismaBuild, just like the green suite.

`crates/prismabuild/tools/lookup_mutants.py` moves the capture scan just before
the withdrawn read. It requires the finish-gap regression to fail after the
done read, then restores the source.

## Fleet verification

`crates/controller/tools/crash_manifest.py` generates five PrismaBuild actions.
Three run the default smoke scenario with the host tags `dl380g10`, `sparky`,
and `sparklina`. One portable action sets `TL3_SIM_FULL=1` for the full matrix.
The other runs `verify.sh` with a separate seed and the smoke default, including
workspace formatting, Clippy, all Rust tests, and the kernel build without
default features. The host and workspace actions also run the existing
controller scenarios, worker process scenarios, and channel-disconnection tests.
The portable full action runs only `crash_safety`.
The simulation prints its own elapsed time and number of real process kills.

The crash matrix uses a small seeded footprint. The tracer's existing 80-file
case remains in the controller suite and runs in every placement too.

Ticket #71's focused final campaign comes from
`python3 crates/controller/tools/loom_manifest.py`. Its three pinned actions run
the bounded models and all twenty crash save points on `dl380g10`, `sparky`, and
`sparklina`. The dl380g10 action also repeats both bell models 50 times at bound two.
A portable action runs `crates/controller/tools/verify.sh`, and another checks
all 13 ordering mutants plus the lookup scan-position mutant. The workspace script also runs the bounded loom suite. Each action reserves two
CPUs and 6 GiB, bounds build and native threads, and preserves PB affinity.
