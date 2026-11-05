# Dispatch liveness: the four outcomes omit permitted waits

Issue #817 cannot be proved as written without changing its domain or its
outcomes. This note stops implementation under the builder's Q27 rule. It does
not propose changing the state table.

The missing-watch defect from #815 and the intact-watch, dead-process defect
remain valid bugs to address. A lost worker maps directly to infrastructure
failure under state-table "Failures", Q7. That mapping is not the blocker.

## Counterexample

Start a controller with two agent-pool grants. Intake three requests while the
first two ingest runs remain live. The third node has this state:

| Field | Value |
|---|---|
| Phase | Ingesting |
| `waiting_on()` | AgentPool |
| Pool ticket | Present |
| Pool grant | Absent |
| Run, job, in-flight effect | Absent |
| Effect sequence | 0; no effect has ever been dispatched for this node |
| Authoritative reply or failure | Absent; its first operation has not started |
| Watch and allowance | Absent |
| Open incident | Absent |

All four requested outcomes are false. Restarting preserves this state. Idle
controller steps cannot grant capacity that another run still owns. This is a
valid settled world under Q39, not missing dispatch evidence.

Next, send a genuine console Pause command for the third request. The same
node now returns Resume from `waiting_on()`. Its first start remains deferred,
with none of outcomes (a) through (d). Restarting and driving do not change it.
Even when the other runs finish, this node must remain paused until an
authorized Resume or another applicable external event arrives. There is no
bound K at which an ordinary paused node becomes one of the four outcomes.

These cases do not rely on corrupt storage, a missing watch, a crashed process,
or exhaustion. No past reply from another operation can stand in for an
authoritative ending of the operation this node has never started.

## Binding rules and executable paths

- `docs/state-table.md`, "Agent pool": a ticket waits for a grant. Higher
  priority arrivals may hold it back indefinitely; starvation is accepted.
  Admission is an internal kernel update, not an effect.
- `docs/state-table.md`, "Facts and commands in every phase" and "Pausing":
  a pause starts no new operation; a non-leaf stores its pending start.
  Resume or release of the pause enables it. A normal pause has no deadline.
- `docs/state-table.md`, "Failures": lost workers are infrastructure failures.
  A normal ungranted ticket or operator pause is not evidence of worker loss.
- `crates/kernel/src/queries.rs`, `Node::waiting_on`: paused nodes return Resume;
  nodes with a ticket and no operation return AgentPool.
- `crates/kernel/src/scheduler.rs`, `advance`: starting needs `!n.paused`,
  `pool_free(w)` and `first_in_line(w, i)`.
- `crates/kernel/src/commands.rs`, `on_command`: Pause sets `paused`; Resume
  clears it. Neither invents a worker outcome or an incident.
- `crates/controller/src/dispatch_liveness.rs`, `acknowledged_wait`: repair
  requires an existing current-generation operation. An ungranted first start
  is intentionally outside that repair path.
- `crates/kernel/src/progress_global.rs`, `resume_decreases`, and #805: an
  external Resume can lower the rank. The theorem does not require the
  controller to manufacture that external event or guarantee per-node fairness.

The effect domain also needs an explicit boundary. The table's delivery
classes include GitHub and worktree effects, which do not have an agent-worker
supervisor process. Outcome (a)'s supervisor-process requirement cannot be
applied literally to every `Wait::Effect` variant.

## Two designs for the lead to resolve

1. Keep the four outcomes for dispatched operations. Prove controller
   coverage of each dispatched operation and prove dependency coverage
   separately for AgentPool and Resume. An intentional pause has its durable
   owner; a pool waiter has its ticket and current grant obstruction. These
   are additional outcomes, so this changes #817's stated property.
2. Keep every listed wait in one classifier. Add typed outcomes for intentional
   pauses, resource waits and non-worker delivery contracts. Prove bounded
   reconciliation into that expanded set without promising fairness. This
   also changes #817's stated property, while preserving the state table.

I prefer the second design because an exhaustive match can reject an omitted
wait variant. Neither design is authorized by the current instruction to
prove exactly the four outcomes without weakening the property.

Forcing every normal AgentPool or Resume wait into the existing four outcomes
would instead need a new failure, incident or timed-retry rule. That requires
Rob's Q27 approval. A timer must not turn intentional waiting into evidence of
failure. Counting an unrelated or historical reply would only hide the gap.

## Evidence and handoff

The runnable counterexamples and remote execution records are retained in
`/home/rob/tmp/tl3-build/rust-fv/t817/`. They append two test-only witnesses to
the existing real-store dispatch-liveness integration test on a disposable
BuildBuddy checkout. Each checks the kernel invariant, absence of an operation,
absence of all relevant durable coverage, repeated idle drives, and restart.
They change no production code and use only disposable Postgres databases.

No reconcile extraction, Verus theorem, mutant, or conformance claim is made.
The #816 P2 and lost-process repair remain unimplemented. Existing #815/#816
regressions remain unchanged. Resume implementation after the lead resolves the
property's domain and outcomes; do not merge a classifier that claims an
ungranted ticket or intentional pause is a lost worker.
