# RP #930 option B measurement draft

found by: proof

This draft starts directly at clone-only canonical `51cd3686d3a70aa96eaa1c4a4bda64ec4f938805`. It is not qualified or integrated. RP remains outside NOW and fullgreen remains false. The parent owns architecture selection and canonical RP.

## Source boundary

Node, Node::new, initial_node, World, Reply and Event return to the retained pre-RP definitions. Dependencies and dependency_retirement are unchanged. ReplanBudget and PlannerResult live in empty_replan, outside those types. No budget enters Credit, Dependency, Outbox or a dependency predicate.

The kernel owns binding, admission and consumption in step_planner. Wrong request, invalid index, wrong phase and stale Await/run inputs return unchanged state and budget before any consumption. Admitted empty-allowed results clear the operation with end_operation before the real consume/refuse helper. First use clears the question and resets the existing Planning agenda with a fresh generation. Repeat use stops for PlanCantMeet review. Both fail_fast values take the same branches. Credit, Q54 replanned and plan_retries are preserved by the helper. The helper emits no output and allocates no question. The entry then drains and settles, matching the ordinary ordering.

Ordinary replies pass the existing accepts/on_reply handling. After consumption, admitted cant_meet, split and nonfit leaves map to PlanCheckRejected. A valid fit, nonempty leaf outside must_split retains the existing accepted transition. Invalid footprint/schema and unrelated failures keep their ordinary handling. Generic step has no empty-allowed replay variant. Integration must send every later planner reply through step_planner, since generic step cannot inspect an external budget.

## Atomic request storage plan, not implemented

The durable location is the existing requests/<NodeId> row. Request construction alone creates a fresh ReplanBudget bound to its append-only NodeId. Replacement versions append a new node and receive a fresh budget. Generation changes, retry, recovery and restart retain the same record. The controller stores the kernel-returned budget and never decides or increments consumption.

Within an apply transaction, read pending request writes first, then snapshot rows. Read World and request rows at the same revision. Invoke step_planner with that typed budget and the row's NodeId. Stage the returned budget into the request row while retaining all request metadata. Stage World, effects and controller bookkeeping in the same existing atomic commit. A second planner event in that transaction reads the staged consumed budget rather than a fresh snapshot value.

On commit conflict, publish neither World nor budget nor effects. Discard both candidate states, read the winning revision and its pending writes, then reapply the event through kernel admission. Do not replay a cached unused budget. If the winning generation advanced, stale admission leaves the permission unchanged. The store must never acknowledge effects from a losing commit.

Legacy import must read the old node row before dropping empty_allowed_replanned. A true legacy flag binds a consumed=true record to that row's NodeId. Missing records with no true legacy flag start unused. If both old and new forms exist, import consumption by logical OR, reject a mismatched request binding, and commit the normalized request and World together. Never reset true to false. This is a required future migration, not a new compatibility layer in this draft.

## Unverified and unwired work

The public entry, helper contracts and adjusted current witness are source drafts. The entry contracts describe first/repeat/stale behavior on the real entry and both fail_fast values, but no new RP proof or witness is selected. No witness/mutant or runtime test is run. The witness still exercises the actual helper; a later authorized entry-level witness must exercise admission, settling and persistence composition.

Controller production protocol.rs and lib.rs still reference Reply::PlanCheckReplan and Node.empty_allowed_replanned, so the controller is uncompilable against this draft until its result translation and apply/store path are wired. Existing controller tests, protocol-rp and rp-current replay copies are also incompatible. They are deliberately not edited. No frozen reference, Kani harness or replay baseline is changed. Kernel safety_qualification.py mutation anchors remain stale and unrun. No broad integration build is claimed.

Later work requires request serialization/legacy import, pending-write reads, atomic conflict tests, all later planner endings, public-entry proofs and witnesses, normal watchdog gates, and complete qualification under separate authorization. The diagnostic measurements here cannot discharge those obligations.

## Authorized evidence

Only dependencies count 1 and the exact 33 reviewed split clone selectors, each count 1, are eligible. No retained baseline is remeasured. One uncached remote campaign uses Bazel jobs16, one Verus/native thread, explicit outer180s and watchdog120s with kill-after5. Runner instrumentation is remote-only after candidate source identities are saved. Production watchdog, solver rlimit and attributes remain unchanged. Raw statuses, logs, source selections, query files and CAS receipts remain evidence even if setup fails. Missing metrics do not establish a resource overrun or authorize option A.
