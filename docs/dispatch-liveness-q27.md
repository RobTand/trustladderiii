# Controller dispatch liveness (#817)

The lead corrected #817's property on 2026-11-05. The original four outcomes
omitted legal agent-pool tickets, authorized pauses and non-worker delivery
contracts. The two real-store counterexamples remain regressions. This was a
property correction, not a state-table change or an outstanding Q27 decision.

The classifier matches every actual `Wait` variant without a wildcard. Its
successful outcomes have explicit evidence:

| Outcome | Evidence and existing state-table rule |
|---|---|
| Dispatched | Worker: matching current-operation watch and live recorded process identity. Other effects: immutable intent under the delivery-class contract. |
| Ending | Current-generation authoritative reply, failure or process termination. Q7. |
| Retry(Allowance) | Current-operation allowance with a durable deadline. Looks and stalls. |
| Retry(ModelQuota) | Current-operation quota allowance with its durable reset. Q53; #828. |
| Incident | Open node incident or global counter-headroom halt. Q7; #771. |
| Paused | Durable authorized pause, without a deadline. Pausing. |
| PoolWait | Ticket with a current obstruction. A full pool requires all its occupants to have operation coverage; otherwise an earlier/higher-priority ticket obstructs. Q39. |
| Answer | Durable current question. Asking Rob. |
| MergeQueue | Queued operation's durable PR binding. Queued phase. |
| SlotOrFiles | Ticket obstructed by slot width, held files or an older overlapping waiter. Q30. |
| Prerequisite | Undelivered merge prerequisite. Planning admission; #693. |
| Admission | Planning admission blocked by the repository's Ready waiters. Q30. |
| Closed | Final Closed node. Closed is final. |

`Silent` is an explicit rejection, not successful coverage. Recovery of an
acknowledged operation reads its original immutable effect, then restores its
watch and allowance, or records the existing dispatch-missing incident. It
never executes another start or constructs a successful reply. The reference
model uses these same executable decisions.

A process-ending wake with no exit record imports `WorkerFailed(LostWorker)`
through ordinary ingress. Q7's existing accounting retries the first failure,
raises an incident on the second, and spends no credit. A complete-looking
stdout without an exit record supplies no reply evidence. Cancelled claims,
late generations and superseded attempts retain their existing fences.

A recorded quota ending already has its retry deadline. It takes precedence
over dead-process evidence. Recovery neither reconstructs a missing watch for
that completed attempt nor turns it into a lost-worker failure. No early quota
wake, reset cap or new incident rule is introduced. The classifier's integration
seam is `quota_retry(until: u64) -> RetryPending`, yielding
`RetryPending::ModelQuota { until }`, where `until` is Unix milliseconds.
Detailed role/model/family/key metadata belongs to #828's quota facts.

Exhaustion deferral persists its worklist and a parking row, covered by the
existing #771 incident. Idle drives, allowance wakes and unrelated row writes
do not rescan headroom. The authorized clearance transaction removes parking;
startup also reevaluates current durable evidence. Restoring a test counter
alone does not clear an open incident.

The pure model's bound is one recovery transaction after the original dispatch
read, or one process-ending ingress transaction. Controller/store conformance
uses K=32 drives for its fixed one-node backlog. Neither bound is a wall-clock
timeout, and neither promises ticket fairness. Production wake sources remain
inotify, pidfds, committed pool changes, startup and existing durable deadlines.

The test-only pre-extraction method is copied verbatim from cdacce8f into
`crates/controller/tests/fixtures/dispatch_recovery_original.rs`. Differential
schedules compare all effect reads, clock samples, incident construction,
commits, effects and persisted bytes, including precommit loss and a lost reply
after commit. Store transactions are atomic; the model does not let individual
rows of a transaction land independently.

Implementation evidence and the final verification inventory are recorded in
`/home/rob/tmp/tl3-build/rust-fv/t817/`.
