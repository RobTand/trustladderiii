# Progress scope and counter exhaustion

Ticket #771 uses the lead's enforced-scope decision. No state-table rule changes.
The progress preconditions are consistency, settledness, an open request,
`World::headroom_ok()`, and `last_cmd < u64::MAX` for every node. The complete
progress theorem and the per-step counter bounds remain proof obligations.
Existing Kani harnesses and tests stay.

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

The pinned Verus toolchain and spike sources remain at the locations recorded in
`/home/rob/tmp/tl3-build/rust-fv/verus-spike.md`. No new Verus result is claimed
by this note. The counterexample runs the real kernel instead.

## Counter audit and proposed step reserves

The apply boundary checks the pure headroom helper before every event. The same
mechanism covers dependency reconciliation's individual `step` calls and startup
capacity reconciliation's `settle_world`. Failed checks raise
`kernel counter headroom exhausted: <counter>` without applying that transition.
The bounds below still need their whole-step proof; passing helper tests alone
does not discharge it.

| Counter | Width | Proposed K per step | Event count needed from a fresh ledger |
| --- | --- | --- | --- |
| `last_cmd` | u64 | 1 per accepted controller command | About 1.84e19 accepted commands on one request. Checked separately at command construction. |
| `generation` | u64 | 3 | At least about 6.15e18 events affecting one request. Apply can increment twice; settle adds at most one phase change. |
| `next_seq` | u64 | 2 | At least about 9.22e18 events affecting one request under this conservative bound. Apply and settle can each emit at most one effect per node. |
| `next_question` | u64 | 1 | About 1.84e19 events globally. Allocations occur only in apply. |
| `next_ticket` | u64 | `2 * (nodes.len() + 1)` | Depends on world size; two tickets per node per settle, including a possible new node. A fixed K independent of node count would be unsound. |
| `credit.start` | u32 | 258 | Initialized once to at most `u8::MAX + RESERVE`; fresh requests cannot exhaust it. |
| `credit.earned` | u32 | 1 | Merge credit applies once per request; fresh requests cannot exhaust it. |
| `credit.added` | u32 | 255 | About 16.8 million maximal AddCredit answers. This includes automatic top-ups, which add at most one for a continuation. |
| `credit.spent` | u32 | 1 | About 4.29 billion paid events on one request. |

Credit's derived arithmetic uses i64, which contains the sum and difference of
all four u32 entries. A consistent ledger is nonnegative. Withheld credit allows
only zero-cost question continuations. For an AddCredit answer, the deposit
covers the one-unit continuation unless the deposit is zero; that case tops up
by one. Thus deposit plus automatic top-up is at most 255, not 256. The local
credit operations have source-level Verus contracts; their callers and the
complete step still need composition proofs.

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
saturating operations. Their arithmetic, the u32 scratch scan/pass counters and
the mathematical whole-world rank still need explicit proof treatment. In
particular, `admits` counts Ready waiters in u32; persistent counter headroom
alone does not bound that count for arbitrary world sizes.

Q30 and Q39 still accept starvation. The theorem concerns the existence of
some progress event, with no per-request fairness claim.

## Proof migration status

No Kani harness is replaced or weakened. There is no gated Verus replacement
in this PR yet. The mapping below records obligations that remain for #771:

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
