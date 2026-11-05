# Progress scope and counter exhaustion

Ticket #771 uses the lead's enforced-scope decision. Kernel transitions are
unchanged. Rob approved the controller's durable halt at counter exhaustion on 2026-11-05
([Q27 approval](https://github.com/RobTand/trustladderiii/issues/771#issuecomment-6648842596)).
The state-table row is **Counter headroom**; global progress is tracked in #805.
The safety contract on the real `step` takes the production checker's invariant,
normalized file storage, and `World::headroom_ok()`. It states the complete
per-step counter budgets and nondecreasing command IDs. Verus also checks each
callee body, including apply, settle and the actual event handlers. It checks
all arithmetic in those bodies against the real integer widths. The controller
checks headroom before an event, dependency reconciliation step, or startup
settle. A step may consume its reserve; the next boundary check then stops it.
Headroom is a precondition for safe arithmetic, not an invariant that reserves
remain untouched after every step.

Global progress is separate. Its intended scope additionally requires
settledness, an open request, and `last_cmd < u64::MAX` for every node. No Kani
harness or test is replaced. The source-connected progress draft is preserved
on `t771-progress-draft-20261104`, head `2668baf75a`.

The remaining progress composition is stuck in `World::dependency_fact` and
`on_fact`. At rlimit 90, focused runs took 168.15 s and 176.21 s and exhausted
the solver limit. Separating key lookup from guards, hiding whole-world
predicates, lifting the exact dependency update separately, and explicitly
instantiating its strict summand proved supporting lemmas but did not discharge
those compositions. The safety scope requested in the lead's checkpoint does not assert the global
outside-witness theorem. [The follow-up issue draft](progress-followup.md) records
the obligations, measured attempts, and proposed next step.

The controller now allocates command IDs through `Node::next_command_id`, using
`checked_add`. Exhaustion raises `command ids exhausted for request N` without
emitting an event. Raw `Observation::Kernel(Event::Command { .. })` is rejected
in `Controller::translate`; typed `Observation::Steering` carries a command but
no caller-assigned ID. It uses the same checked constructor. Thus accepted
controller commands advance `last_cmd` by exactly one. Exhaustion from zero
requires 18,446,744,073,709,551,615 accepted commands on one request. This is a
physical reachability argument, not an unbounded mathematical invariant.

The raw kernel still accepts any ID greater than `last_cmd`. The following
counterexample characterizes that unrestricted API and lies outside the theorem
scope. It remains a regression test. The witness now returns `None` at command
exhaustion, with its signature unchanged, instead of panicking or wrapping.

## A reachable settled world has no progress event

The fixture in `tests/progress_limits.rs` uses only public `step` calls, starting
from `World::new(&[1], 1)`. It never edits a node or invents an abstract transition.

1. Create an ordinary request and confirm its ingest start effect.
2. Reply `IngestDone { size: 1 }` and confirm its audit start effect.
3. Send `Command::Pause` with command ID `u64::MAX`.
4. Reply `AuditAgrees` to the already running audit.

Every resulting world passes `World::valid()`, which includes consistency and
settledness. The final world has one Unplanned, paused root. It has no effect,
run, question, incident, PR history, or dependency. Its command ID is exhausted.
Its measure is `28 * 64 + 2 + 8 = 1802`.

Before the authorized fix, `World::witness` selected Resume and evaluated
`last_cmd + 1`. Debug overflow checks panicked; release arithmetic wrapped to
zero, which `on_command` ignored. The checked helper now returns `None`.

This is more than a bug in the witness selector. No `u64` command ID is greater
than `u64::MAX`, so every command is ignored, including Stop and ChangeRequest.
The other event variants cannot lower this world's measure either:

| Event | Reason it cannot lower the measure |
| --- | --- |
| NewRequest, NewAdoption | Append an open node with positive distance. |
| Reply | The root is at Admit, with no run or job awaiting a reply. |
| Ack | There is no effect to acknowledge. |
| Fact::Dependency | There is no existing dependency to deliver. A new declaration cannot reduce the count. |
| Fact::DependencyRemove | Removal applies only in Ingesting. |
| Fact::Priority | Priority does not change distance. |
| Other facts | Their phase, PR-history, or incident guards are false. |
| Command | Every representable ID satisfies `id <= last_cmd`. |

Settle also cannot move the paused root. A different rank cannot supply the
missing command or make the unchanged state strictly decrease.

The control fixture uses `u64::MAX - 1`. Its actual Resume witness has ID
`u64::MAX`, and applying it strictly lowers the whole-world measure. The
counterexample needs one root, one repository, and six outside events. It does
not depend on a huge world or a long replay.

## What a ghost rank can and cannot fix

A mathematical sum avoids overflow in `World::measure`. It does not extend
the executable command-ID domain. Adding `last_cmd < u64::MAX` as an unproved
premise would exclude the accepted, reachable counterexample above. The current
Kani fixtures already exclude it through their `last_cmd < 2^40` assumption.

The #764 Target A projection represents a fresh command as an enabled abstract
Resume. Refinement must discharge that freshness premise against `on_command`.
At the exhausted ID it cannot. Proving more gate or phase lemmas over that
projection would not repair this failed correspondence.

## Verification entry points

The pinned Verus toolchain and spike sources remain at the locations recorded in
`/home/rob/tmp/tl3-build/rust-fv/verus-spike.md`. `verus/run.sh` is the
PrismaBuild exploratory runner. The required Rust targets are the 17
`//crates/kernel:verus_*` groups declared in `tools/bazel/verus.bzl`. Their exact
positive inventory totals 256 verified obligations, including generated type
obligations. Every actual callee of `step` is covered by one group. The runner
rejects missing, zero, duplicate or changed counts, nonzero exits and timeouts.
No `assume`, `admit`, external body or alternative transition model supplies
these contracts.

`tools/safety_qualification.py CASE` runs on a disposable BuildBuddy runner. It
first requires the normal target to pass, changes one expression in the real
source, and requires that same target to fail an executed assertion or a proof
obligation. It also checks that the Gate classifies that failure as a failed
required Rust check. Compiler errors and timeouts do not qualify a mutant.

The cases cover the headroom predicate, actual question increment, command
allocator, command-order guard, raw-event validator, steering constructor,
controller apply guard, and actual translate call. The apply and translate
cases run the controller's database-backed regressions; the other six run the
required Verus targets. This connects the proved pure guards with their actual
controller call sites. The hand-off evidence records each BuildBuddy verdict,
proof count, timing, memory and mutant result.

## Counter audit and step reserves

The apply boundary checks the pure headroom helper before every event. The same
mechanism covers dependency reconciliation's individual `step` calls and startup
capacity reconciliation's `settle_world`. Failed checks raise
`kernel counter headroom exhausted: <counter>` without applying that transition.
The input stays queued. One existing counter incident halts further kernel
inputs across wakes and restarts until authorized clearance. Clearance does not
restore numeric headroom; an operator repair or release must do that first.
Q51 also clears this TL3-side halt once on a new release; a repeated failure
raises a new incident, while a same-release restart leaves it open.
Trusted internal acknowledgements and deferred intake use private durable inputs
and share arrival order with the inbox. Rob's clearance can pass the retained
queue through the existing operations-comment path. The **Counter headroom** state-table row records Rob's
[Q27 approval](https://github.com/RobTand/trustladderiii/issues/771#issuecomment-6648842596).
Complete issue edits join that queue before timestamp or priority filtering.
Each complete inbox input has one store transaction, opened before translation.
It includes interning, queue receipts, classifier jobs and watches, request and
intake rows, completion keys, dependency announcements, the world and input
consumption. Helper commits release savepoints inside that transaction. If any
step refuses, the outer transaction rolls back before a separate transaction
records the deduplicated counter incident. The complete input remains queued.
Complete direct inputs use the same boundary, including acknowledgements,
cancellations, replacements, priority rereads, intake and issue edits.

The real-store refusal property inventories every observation variant and each
private input kind, refuses before every real kernel step and after the final
SQL writes, and checks every persisted table and sequence. After clearance and
restart, replay must match an uninterrupted run. The comparison removes only
incident audit receipts and each incident's single arrival allocation, maps
their revision offsets out of identifiers and references, ignores SQL
wall-clock timestamps and compares arrival ordering. All other allocator
high-water values and all legacy sequences are compared exactly on refusal.
Fixed regressions retain the Queue(Merged), Classified and queued-intake edit
failures from review. Deterministic proptest seeds and persisted regressions
make the generated inputs repeatable.

PostgreSQL sequences do not roll back. Migration 22 therefore supplies a locked,
transactional high-water row for IDs allocated during input processing,
including the phase-history trigger. Existing sequence values remain allocation
floors. An aborted allocation leaves neither a row nor a sequence gap, and
deleting a consumed input cannot reuse its committed arrival number. The world
column's SQL default uses the same allocator for direct store tooling inserts.

The external-write audit found wait-subscription removal inside input handling.
It now stages a durable retirement intent. Only a committed intent may call
`watch_wait(delivery, None)`; a failed call waits for its incident's clearance.
Other input translation adapter calls read prerequisite or issue evidence and
capture diagnostics. Effect dispatch, cancellation, adapter retirement and
worker cleanup run outside input translation, after their durable intent.
The contracts on `step_with_passes`, `apply`, `settle`, and their actual
handlers establish the bounds below. Helper equivalence alone would not do so.

| Counter | Width | K per step | Event count needed from a fresh ledger |
| --- | --- | --- | --- |
| `last_cmd` | u64 | 1 per accepted controller command | About 1.84e19 accepted commands on one request. Checked separately at command construction. |
| `generation` | u64 | 3 | At least about 6.15e18 events affecting one request. Apply can increment twice; settle adds at most one phase change. |
| `next_seq` | u64 | 2 | At least about 9.22e18 events affecting one request under this conservative bound. Apply and settle can each emit at most one effect per node. |
| `next_question` | u64 | 1 | About 1.84e19 events globally. Allocations occur only in apply. |
| `next_ticket` | u64 | `2 * (nodes.len() + 1)` | At least about 4.61e18 events before the conservative headroom check can fail from an empty world. See the charging argument below. |
| `credit.start` | u64 | 258 | Initialized once to at most `u8::MAX + RESERVE`; fresh requests cannot exhaust it. |
| `credit.earned` | u64 | 1 | Merge credit applies once per request; fresh requests cannot exhaust it. |
| `credit.added` | u64 | 255 | 72,340,172,838,076,673 maximal AddCredit answers. This includes automatic top-ups, which add at most one for a continuation. |
| `credit.spent` | u64 | 1 | About 1.84e19 paid events on one request. |

Credit's derived arithmetic uses i128, which contains the sum and difference of
all four u64 entries. Serialized old u32 values remain valid JSON integers.
The controller preserves large signed journal deltas as several i64 rows in one
atomic commit, with their exact sum unchanged. A consistent ledger is nonnegative. Withheld credit allows
only zero-cost question continuations. For an AddCredit answer, the deposit
covers the one-unit continuation unless the deposit is zero; that case tops up
by one. Thus deposit plus automatic top-up is at most 255, not 256. The local
credit operations and their callers share source-level Verus contracts. The
full-step contract composes those bounds with settle.

No remaining raw observation assigns a monotone counter. Reply generation and
acknowledgement sequence numbers are compared with existing values. Ingest's
u8 size initializes credit.start to at most 258 once. Answer question IDs select
an existing question, and AddCredit is u8. Head IDs, prerequisite issue/PR
numbers and target request IDs are identities, not monotone kernel counters.

Production raw observation sources inspected:

- `adapters/workers.rs` and `adapters/workers/fake.rs`: worker Replies.
- `adapters/prismabuild.rs`: check-result Replies.
- `adapters/worktrees.rs`: worktree Facts.
- `adapters/github/publishing.rs` and `adapters/mergify.rs`: PR/queue Facts.
- `looks.rs` and executor completion forwarding in `lib.rs`: Facts and forwarded
  observations, all passing through translate.
- `ingress.rs`: simulation stdin forwards observations to observe, which now
  rejects raw Commands as well as raw Ack/NewRequest/NewAdoption.

The u8 attempt, round, retry and incident fields are bounded by consistency or
saturating operations. Their arithmetic is checked in the actual apply-side handlers. The helper also scopes the u32 scratch scans and settle-pass counter.
It reserves `open_nodes + 1` scan entries and `6 * (open_nodes + 1)` moves,
including one possible new root. This admits at most 715,827,881 open nodes.
Closed nodes may be arbitrarily numerous within the checked ticket reserve.
The existing Kani worlds have at most four nodes, so this enforced bound keeps
the current guarantee. It is an explicit cardinality scope, not a claim about
an unrestricted finite world.

Each local settle case proves a decreasing height at most six and allocation
potentials at most one generation, one effect, two tickets and one merge credit.
These statements quantify over every scheduling decision, with no world-size
bound. Both normal Rust and Verus compile those same case bodies. The read-only
pool occupancy, planning-admission, slot/file and prerequisite queries verify
against exact predicates. The actual scheduler and settle loop compose the
local potentials into the whole-step counter budgets.

For global tickets, the per-step reserve scales with node count, but the lifetime
count still needs about 2^64 events. Charge an allocation against the local ticket
potential. Settle consumes that potential, while one apply can raise it by at
most two on its selected node, or append one new node with potential one.
Starting empty, after E events `next_ticket <= 2E` and `nodes.len() <= E`.
Thus the reserved total is at most `4E + 2`. Exhausting it requires about
4.61e18 events. This is the charging argument to formalize when proving apply's
frame and composing the local contracts; it is not yet a delivered theorem.

Q30 and Q39 still accept starvation. The theorem concerns the existence of
some progress event, with no per-request fairness claim.

## Proof migration status

No Kani harness is replaced or weakened. The safety proof does not replace a
progress harness. Widening credit requires type-only changes in `proofs.rs`: the
numeric credit bound remains `1 << 30`, and all assertions and covers remain.
The mapping below separates delivered safety from remaining progress obligations.

| Safety foundation | Executable connection | Progress still required |
| --- | --- | --- |
| `world_headroom`, `node_headroom`, `open_prefix` | `headroom_ok`, `counter_exhaustion`, checked scan/settle/ticket budgets | None for counter safety; progress also needs fresh commands. |
| `internal_height`, allocation capacities, `advance_post` | Every executable `advance_node` case and its dispatcher | Outside-event witness decrease. |
| `potential_update`, `potential_bound`, `settle_reserve_totals` | Potentials over sequences of the real `Node` type | Strict global outside-witness decrease. |
| `pool_users`, `slot_waiters`, `prerequisites_delivered` | Actual pool, admission and prerequisite queries | Prove slot/file obstruction and global witness existence. |
| Allocation and credit contracts | Actual `set_phase`, `emit_node`, `take_ticket_node`, `ask_rob`, `raise_incident`, retry and credit bodies | Connect successful outside witnesses to strict rank decrease. |

| Existing Kani harnesses | Required Verus result, not yet delivered |
| --- | --- |
| `progress_from_ingesting`, `progress_from_auditing`, `progress_from_planning`, `progress_from_making`, `progress_from_checking`, `progress_from_reviewing`, `progress_from_publishing`, `progress_from_queued`, `progress_from_rebasing`, `progress_from_awaiting_rob`, `progress_from_stopping`, `progress_from_closed` | Source-connected per-phase witness decrease and global composition. |
| `progress_from_unplanned`, `progress_from_unplanned_delivered`, `progress_from_unplanned_delivered_one` | Source-connected Unplanned direct progress, admission obstruction, and global composition. |
| `progress_from_unplanned_waiting`, `progress_from_unplanned_waiting_one` | Dependency-blocked admission and holding restrictions with arbitrary prerequisite counts. These harnesses do not assert a progress witness. |
| `progress_from_ready` | Gate and pool obstruction reaches an actual witness or internal move. |
| `gate_progress_w2`, `gate_progress_w3`, `gate_waiting_progress_w2`, `gate_waiting_progress_w3` | Arbitrary-size gate witness-or-internal-move composition. |
| `gate_maker_progress_w2`, `gate_maker_progress_w3`, `gate_merge_progress_w2`, `gate_merge_progress_w3`, `gate_resume_progress_w2`, `gate_resume_progress_w3` | Executable event refinement and whole-world rank decrease. |
| `gate_leaf_progress_w2`, `gate_leaf_progress_w3` | Q48 grant after the last conflicting holder finishes, with older tickets and disjoint overtakers. |

The counterexample tests characterize the unrestricted API in debug and release
profiles. The final proof must refine the actual `apply` and `advance` bodies,
not the #764 projection. Importing the kernel or verifying helper contracts alone
does not complete that refinement. Seeded mutants must reject each delivered
lemma before any replacement of the corresponding Kani harness is proposed.
