# One subject and observer proof rule

Issue [#645](https://github.com/RobTand/trustladderiii/issues/645) approves this Q27/Q55 change. Rob's decision on 2026-11-03 was: "You may update the proof rule accordingly so that fixes to flaky tests are allowed."

## The proof unit

A check identifies subject source separately from the observer that judges it. The subject is repository code whose behavior changes. It can be production code, a decorated helper, a class, a test, or fixture code. The observer is the selected pytest assertion code, or the trusted pytest outcome observer for an existing test being repaired. Existing check kinds stay unchanged.

The check gains one source declaration, `proof`, with `subjects` and `observer_support`. Subjects name relative Python files or qualified definitions, such as `tests/test_queue.py::_execute` or `tests/world.py::World`. Existing `node_ids` identify the observers. `observer_support` names new definitions or new modules needed by those observers. It contains source identities, never executable strings or argument recipes. Subjects may span modules and may change signatures, decorators, or implementation dependencies.

For ordinary demonstrations without an explicit declaration, the harness treats the unchanged base outside the selected head observer definitions as the subject. It records that inferred partition. Explicit declarations make a test-file subject visible to planning, maker instructions, and review, and must refer to real changes. For a forced or declared-symptom check without explicit subjects, the selected original tests and fixtures are the subject and the trusted pytest reporter is the observer. This retains the approved original-test symptom campaign. An explicit declaration can instead combine forcing with new head observers of an old helper.

Source obligations remain separate from selected methods. A quoted source requirement cannot disappear when the planner changes methods. A request for helper diagnostics does not itself require the old helper driver. Old source requirements for removed methods receive a precise unsupported-method refusal rather than silent substitution.

## One assembly function

`assemble_proof(base, head, destination, check, configuration)` in the trusted harness owns RED staging and its source receipt. All three callers use it: planner rehearsal through `pb_tests`, maker `run_tests(base=True)`, and independent `run_checks` in Checking. The planner has no future head observer yet; rehearsal supplies base as both source trees and records the same source partition. It still proves only forcing applicability before maker work. Checking proves final RED/GREEN independently.

Assembly starts with the real base tree. For assertion observers, it copies only the selected head observer definitions, including their decorators, into their base modules. It retains every other base definition and import. The assembler carries head module-level imports whose bound names an observer references and the base lacks. New helper or fixture dependencies require explicit `observer_support`; an existing subject binding cannot be introduced again as support. An unresolved name produces a typed refusal naming the symbol and module through the existing repair boundary, without a new work retry. A new observer module can carry its imports and setup, but cannot replace an existing base module. Existing observer methods retain their class context. Ambiguous definitions, overlapping subject and observer units, path escapes, symlinks, and unsupported partitions produce named proof refusals.

For an existing-test outcome observer, assembly leaves the base subject untouched. GREEN always runs the same check against the actual head. A forced check also requires an ordinary head pass. Regression and behavior-preserving checks retain their frozen-test guards. Configuration and protected-path permissions still apply, including the existing conftest guard; identifying fixture code as subject does not grant permission to edit a protected file.

The receipt binds the base and head revisions, the staged tree identities, subject source hashes, observer source hashes, support declarations, selectors, forcing, and declared symptom. Worker action identity includes the projection and both input trees. A result from different source, selectors, forcing, or projection cannot be reused. The reporting path carries this record to review without reconstructing it from prose.

## Forcing the repository's own reference

A dotted patch such as `prismabuild.pool.time.monotonic` may resolve through an imported stdlib module. The runner must replace the repository module's local reference with a private proxy carrying the patched attribute. It must never mutate the shared `time` module. The proxy delegates unchanged attributes and disappears after the check. Direct global targets, imported aliases with no repository-local attachment point, and paths outside the served repository remain refusals. Static preflight and runtime validation agree on that distinction. Existing reachability, fork accounting, and tamper checks remain binding.

The authoritative forcing contract exposes the per-call delay maximum, aggregate per-test delay maximum, sharing across targets and forks, and reset boundary. Reports retain applied delay and exhaustion separately from invocation counts. Increasing the number of patches cannot increase the aggregate ceiling.

## Deletions and alternatives

Delete `HelperEvidence`, `helper_test_edits`, `helper_failure_preservation`, the helper-target shape classifier, and their helper-only result fields and capability names. Delete literal argument cases, `$attributes`, `$setup`/`$call`, fixture injection, and the generated helper invocation driver. Their schema and prompt sections disappear in the same change. Existing invariants belong in ordinary regression assertions written in repository pytest context. An explicit proof declares the assertion message that supplies RED, so a different assertion cannot satisfy it. The harness must not freeze an obsolete failure predicate that the approved fix removes.

I considered overlaying head test files and restoring declared base subjects afterward. That makes omission dangerous: an undeclared helper fix can remain on base, and a restore can miss changed imports or bindings. Starting from base and adding named observers is smaller and preserves base code by construction. The cost is explicit declarations for new observer helpers or fixtures. Imports are derived from observer references. This is preferable to another partial Python object constructor.

## Verification and scope

The replay corpus records saved request and trace identities for 178, 195, 202, 206, 216, 229, 230, 233, 234, and 225. Small executable specimens preserve the deciding setup, claimed-row, decorated cross-module, local-clock, and negative source-obligation facts. The environment refusal in 229 stays a refusal. Controls reject an overlay carrying the subject fix, shared stdlib mutation, repository escapes, and failure at the wrong node, phase, or exception. A synthetic wrong assertion must not satisfy a declared reason.

Commit the corpus first and show intended failures on main through BuildBuddy. Then implement the shared path and delete the superseded encodings. Run narrow worker, harness, and controller targets on the final merged head, with receipt checks. Report production and total line deltas separately. No kernel type or transition change is needed, so Kani is unnecessary unless implementation disproves that premise. Fail-fast, retry, credit, incident, and restart rules remain unchanged.
