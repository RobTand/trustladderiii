# Command ID exhaustion blocks unbounded kernel progress

Ticket #771 cannot meet its unrestricted progress claim without a decision about
counter exhaustion. This note records a counterexample in the executable kernel.
No kernel rule, type, or transition has changed. All existing tests and Kani
harnesses remain.

The binding builder brief says, "No state-table rule, kernel type or transition,
and no Q54 change; if one is needed, STOP and hand back a design note for a Q27
decision." This is that handback. It does not complete #771.

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

`World::witness` selects Resume and evaluates `last_cmd + 1`.
With overflow checks, that panics. With wrapping arithmetic, the witness has
command ID zero. `on_command` ignores it because `id <= last_cmd`.

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

## Decision needed before implementation resumes

The lead needs a Q27 decision about the progress contract and counter lifecycle:

- Specify a concrete exhaustion or renewal protocol, then authorize its kernel
  behavior and prove its refinement. Changing only addition to saturate or wrap
  does not make a greater command ID available.
- Or explicitly scope the theorem to worlds with enough counter headroom, and
  decide how the executable system enforces that scope. This changes the
  unrestricted no-dead-state claim and is not silently added by this PR.

Generation, effect-sequence, ticket, question, and credit arithmetic also need
their own obligations. This note establishes one sufficient blocker. It does
not claim to have audited every counter.

Q30 and Q39 still accept starvation. This counterexample concerns the existence
of any progress event, rather than fairness or progress for an individual waiter.

## Proof migration remains blocked

No Kani harness is replaced or weakened. There is no gated Verus replacement
in this PR. The intended theorem mapping remains work for #771 after the decision:

| Existing Kani harnesses | Required Verus result, not yet delivered |
| --- | --- |
| `progress_from_ingesting`, `progress_from_auditing`, `progress_from_planning`, `progress_from_making`, `progress_from_checking`, `progress_from_reviewing`, `progress_from_publishing`, `progress_from_queued`, `progress_from_rebasing`, `progress_from_awaiting_rob`, `progress_from_stopping`, `progress_from_closed` | Source-connected per-phase witness decrease and global composition. |
| `progress_from_unplanned`, `progress_from_unplanned_delivered`, `progress_from_unplanned_delivered_one` | Source-connected Unplanned direct progress, admission obstruction, and global composition. |
| `progress_from_unplanned_waiting`, `progress_from_unplanned_waiting_one` | Dependency-blocked admission and holding restrictions with arbitrary prerequisite counts. These harnesses do not assert a progress witness. |
| `progress_from_ready` | Gate and pool obstruction reaches an actual witness or internal move. |
| `gate_progress_w2`, `gate_progress_w3`, `gate_waiting_progress_w2`, `gate_waiting_progress_w3` | Arbitrary-size gate witness-or-internal-move composition. |
| `gate_maker_progress_w2`, `gate_maker_progress_w3`, `gate_merge_progress_w2`, `gate_merge_progress_w3`, `gate_resume_progress_w2`, `gate_resume_progress_w3` | Executable event refinement and whole-world rank decrease. |
| `gate_leaf_progress_w2`, `gate_leaf_progress_w3` | Q48 grant after the last conflicting holder finishes, with older tickets and disjoint overtakers. |

The final tests characterize the defect so the PR can have a green Gate without
claiming to fix it. The red commit asserts the requested progress property and
fails. The final commit explicitly replaces that false assertion with a
counterexample assertion. This is a blocked-contract handback, not a red-to-green
implementation of the requested theorem. No test present on main was changed.

Run the characterization through PrismaBuild in both ordinary and release test
profiles. Record each action's verdict, complete ending, and log digest in the
PR body. Expected failures have failure endings rather than success receipts.
Existing kernel suites and the Kani compile check must still pass.
