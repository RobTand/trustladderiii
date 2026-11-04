# S3 needs a decision about visible duplicates

Issue #747 asks S3 to prove that every effect key has at most one visible
upstream mutation, despite lost responses and duplicate deliveries. The current
comment contract permits two visible copies until reconciliation deletes one.
Implementation stops here under the builder brief's Q27 rule. This note does
not change the state table or propose a production patch.

The source references below were inspected at
`e3292d911a6e42c89f1f197b90648419a39a6b3f`.

## The requested invariant excludes permitted behavior

[The effect machine](state-table.md#the-effect-machine) explicitly promises
deduplication rather than exactly-once delivery. Its comment contract says:

> At least once, then deduplicated

> In a rare race, a message can post twice. The next reconciliation deletes the extra copy.

These are distinct properties:

- #747's safety property forbids a second visible mutation at every reachable
  state, including before reconciliation.
- The existing comment contract permits that state and repairs it later.
  Deleting a duplicate cannot undo the earlier violation of the safety property.

An abstract upstream with an atomic, deduplicating `applied[key]` bit would
exclude the permitted comment race. A passing model under that assumption
would not establish the requested property for comments.

## Source evidence and a bounded witness

| Transition | Existing code path |
| --- | --- |
| Commit Sent before dispatch | `crates/controller/src/lib.rs:1192`, `Controller::drive_once_with` |
| Check for a marked comment, then POST | `crates/github/src/client.rs:677`, `GitHub::post_formatted_comment` |
| Persist two comment copies and lose the response | `crates/controller/tests/crash/upstreams.rs:246`, crash fake `Executor::execute` |
| Leave the effect Sent after a transport error | `crates/controller/src/lib.rs:1255`, `Controller::drive_once_with` |
| Move Sent to Uncertain and reconcile the original key | `crates/controller/src/lib.rs:1271`, `Controller::drive_once_with` |
| Keep the oldest marked app comment and delete extras | `crates/github/src/client.rs:697`, `GitHub::reconcile_comments` |
| Reduce the crash fake's copies from two to one | `crates/controller/tests/crash/upstreams.rs:284`, crash fake `Executor::reconcile` |

The existing contract admits this witness within #747's bounds of one effect,
at most two lost responses, and at most one duplicate:

1. The controller commits `Sent` for key K.
2. A comment is posted with K's marker. A duplicate creates a second visible
   copy. The response is lost. The crash fixture represents this state directly
   with `copies = 2`, then returns an unknown-outcome error.
3. The controller still has the same pending key. There are two visible
   comments, so the requested invariant already fails.
4. Reconciliation retains the oldest comment and deletes the extra copy.
   The visible count becomes one, as the current contract requires.

This is a source-derived witness, not a Stateright counterexample or a newly
observed production incident. No model checker was run for this note.

The existing adapter test
`reconciliation_deletes_only_duplicate_app_comments_and_checks_all_pages`
in `crates/github/tests/rest.rs:224` supplies two app comments with the same
marker on the second page. It requires deletion of the newer comment and
retention of the older one. [The crash simulation](crash-simulation.md#scenarios-and-replay)
also documents the lost-response, two-copy behavior.

## Decision required before implementation

The program lead needs to resolve which guarantee S3 must check:

1. Preserve #747's universal safety property. Comments then need a stronger
   delivery contract and an upstream mechanism that prevents a duplicate from
   becoming visible. That requires a Q27 decision and a separately authorized
   behavior change. Read-before-POST plus later deletion does not provide it.
2. Preserve the current state-table contract. Amend #747 explicitly to separate
   at-most-once effects from converging effects and deduplicated messages.
   For messages, check eventual convergence to one visible copy after pending
   deliveries finish and healthy reconciliation runs fairly. This changes
   #747's acceptance property and must not be done silently by the builder.

Any model must state its scheduling and upstream assumptions separately from
the checked properties. A real-trace conformance test must retain intermediate
states, including the two-copy state, rather than compare only final records.
The credit ledger in `tl3_credit` and the per-key upstream delivery records
also have different purposes and must not be represented as one dedupe table.

No tests are pruned. The S3 candidates remain simulation
`lost_response_keeps_the_key`, `sent_but_absent`, and
`retry_uses_the_kernels_new_key`; Mergify lost-response and refusal-bound
scenarios; GitHub notice retries; Hindsight's lost-ack retain scenario; and
publishing lost-response recovery. Adapter-specific assertions still need
contract tests after an approved S3 model exists.

No Stateright dependency, Cargo package, kernel change, state-table edit, or
workflow change is included. Red/green model evidence, exhaustive state counts,
and model timing remain pending the contract decision.
