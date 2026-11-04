# Transient Mergify dequeue retries: Q27 decision needed

Status: proposed, not approved or implemented. This is the stop-and-report
outcome requested in [#786](https://github.com/RobTand/trustladderiii/issues/786).

## Why this needs a rule decision

Increasing the adapter's one-command budget does not itself require a kernel
change. The requirement that a transient dequeue **never raises an incident
before its retry bound**, however, needs an exception to the existing stall
rule. Implementing that exception in the controller would still change a rule.

The current sources say:

- [State table, Looks and stalls](../state-table.md#looks-and-stalls):
  "Mergify and GitHub: the stall raises an incident."
  Progress means "Checks finishing, or the queue moving."
- [Queued](../state-table.md#queued): a GitHub or Mergify stall, with no incident
  already open, raises one. A probe or Rob clears the first; only Rob clears
  later incidents.
- [Failures, Q7](../state-table.md#failures): "Every second failure raises an
  incident, and the operation waits for \"incident cleared\" before it retries."
  Delivery failures follow the same rule, counted per effect.
- [Q27](../state-machine.md): "A rule change needs Rob's approval, including a
  rule that comes from a bug Kani or a test finds. A clarification doesn't."

There is a concrete conflict even without deciding whether every transient
dequeue is a counted Q7 infrastructure failure. Consider a confirmed transient
dequeue followed by a 60-minute backoff, with no later upstream progress. The
default Mergify no-progress allowance is 30 minutes. Before the backoff ends,
the existing rule requires a stall incident. #786 requires no incident before
the bounded retry total is exhausted. Config permits any positive allowance,
so choosing shorter backoffs does not establish that guarantee either.

Changing the diagnostic's attempt count or advancing the local timer cannot
stand in for upstream progress. Extending the allowance or suppressing stall
handling until the retry bound changes when an incident is required.

## Existing implementation and timer

These locations are on the stacked branch after merging `origin/rust-fv` at
`d6df228c4b0eec7decbc0c6510da75e4cfb9cdd9`:

- `crates/config/src/lib.rs:638`: the default GitHub/Mergify no-progress
  allowance is 1,800 seconds. Validation at line 1132 permits positive values.
- `crates/controller/src/looks.rs:817`: `Look::Alive` compares the progress
  checkpoint and preserves failed-look counts after the allowance expires.
- `crates/controller/src/looks.rs:982`: `look_alive` emits
  `Fact::UpstreamStalled` for GitHub/Mergify after no progress. For a pending
  enqueue effect it also enters failed-look incident handling.
- `crates/kernel/src/lib.rs:2548`: a stall in Queued raises the existing
  incident, with later incidents requiring Rob. In Publishing this fact does
  nothing; the pending-effect failed-look path still applies.
- `crates/controller/src/looks.rs:461`: `next_look_in` selects the earliest
  durable allowance `due_ms`. `crates/controller/src/ingress.rs` waits on that
  timer and calls `look_due`; a retry must reuse this mechanism.
- `crates/controller/src/adapters/mergify/waiting_requeue.rs:371`: recovery
  grace uses a persisted timestamp and subsequent reads through the existing
  look mechanism. It does not exempt Mergify from stall incidents.
- `crates/controller/src/looks.rs:882`: `Look::Quota` persists a reset time
  and schedules the next look. #759's short model-unavailable probe uses this
  model-quota path. Q53 explicitly exempts model quota from failure counting;
  it grants no equivalent exception for Mergify dequeues.

No new sleep, poll loop, kernel fact, or kernel transition is needed to
implement the proposed policy. Approval is needed for its behavior, regardless
of which module implements it.

## Proposed narrow exception for approval

Treat an authoritative, typed transient Mergify dequeue as an internal retry
of the same enqueue operation, in Publishing or Queued. During its bounded
backoff and command reconciliation, do not count that dequeue as a Q7 failure
or raise the Mergify no-progress incident solely because it is waiting on this
retry. Preserve incidents caused by independent failures to read upstream.

Limit the exception to verified machine codes for `BranchIndexingDelay` and
`DraftPrCreationFailed`. Preserve today's behavior for checks failures,
conflicts, person-dequeues, and `Unknown`. In particular, prose that resembles
a transient reason must remain `Unknown`.

Proposed retry policy, to make the decision concrete:

- Allow at most six automatic requeue commands per repository, PR, and head,
  separate from the existing durable command budget. Persist the attempt
  reservation and unique command key before dispatch. A send with an unknown
  outcome counts as reserved and must be reconciled before another command.
- Use backoffs of 5, 15, 30, 60, 60, and 60 minutes. Persist the deadline and
  schedule it through allowance `due_ms` and the existing ingress timer.
  Restart or an early webhook must not shorten or restart the backoff.
- Count a new attempt only after a distinct, authoritative transient dequeue
  of the preceding queue attempt. Repeated reads of one dequeue consume no
  additional attempts. Active, merged, or unresolved attempts do not establish
  another failure.
- If the sixth requeue is authoritatively dequeued for a transient reason,
  raise one exhaustion incident through the existing incident mechanism.
  Repeated looks and restarts must deduplicate it. Do not reset the exhausted
  ledger on incident clear unless that reset is explicitly approved.
- Retain the typed reason, attempts reserved, deadline, and upstream attempt
  identity in diagnostics. Head changes get a separate ledger and must still
  satisfy the existing approval, pause, human-hold, and epoch fences.

The decision needed is whether to approve this bounded Mergify exception to
Q7 and the no-progress rule. Without that exception, the implementation must
retain earlier stall incidents and cannot satisfy the requested invariant.

## Typed evidence still needed

Read-only inspection of PB #2794's head
`1dcd11d4` returned Mergify queue check
[126517983421](https://api.github.com/repos/RobTand/prismabuild/check-runs/126517983421).
Its output has a title and summary describing branch indexing delay, but no
`mergify.v1` span, JSON reason, or abort code. `output.text` is null. Its queue
rule check, `126509983401`, likewise contains prose only. This is evidence
about these API responses, not a claim that Mergify has no typed API elsewhere.

Under #786's typed-only constraint these responses remain `Unknown`. A verified
typed payload or documented upstream endpoint is needed before selecting the
transient wire codes. Do not invent codes from the title. Pin the eventual
payload as a parser fixture and retain the prose-only response as a negative
case.

## Verification after approval

Write and execute the three requested adapter regressions before production
changes: backoff then successful requeue without an incident, exhaustion with
one incident, and unchanged checks-failed handling. Extend the existing frozen
reference differential I/O schedules to allow divergence only for confirmed
transient classes, including crashes at each write and unresolved sends.

Add Kani step proofs for the retry bound, escalation boundary and deduplication,
unchanged command budget, and nontransient equivalence against an independent
oracle. Each needs covers and an intended-assertion mutant. The timer integration
also needs a deterministic test that spans the ordinary no-progress deadline,
an early webhook, and a restart.

No production files, state-table rows, Q-rules, tests, or runtime state changed
for this note. No tests or proofs were run: implementation stopped at the Q27
boundary. No BuildBuddy result is claimed for #786.
