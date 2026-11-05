# G2a2 recorded translate boundary

The old WorktreePrepared and Checks arms are verbatim from 751c00acfcfce94f6cebf62447776e0a44b3e520. The old CheckPlan arm is pinned to the pre-#838 production arm at aa391e4933, recorded in pins.json.
Current copies are compared byte-for-byte with production by the pinned test.
Each side independently compiles its complete arm plus the unchanged head,
request, commit_sha and decode_stored functions. Unchanged maker-input, goal,
evidence, selected-failure, sensitivity and clipping helpers are real production
source closures, with independent hashes checked against production and the base.

The smallest seam is the Observation match arm. The wrapper stops immediately
before the unchanged shared finding/feedback tail. For Checks it captures the
event and all three local feedback values consumed by that tail. WorktreePrepared
and CheckPlan return through their real early returns. The existing simulation
target checks integration through the actual Controller and shared tail.

The recorded store has a global intern dictionary, SELECT/INSERT trace and
per-call read tapes. Repo identities are case-folded, commit identities validated,
and paths interned in order. Pending intern changes and ordered row pushes become
durable together at input commit; crashes before/after each INSERT and commit
compare calls, results, staged rows and persisted logical state. This models
the input transaction, not PostgreSQL storage bytes or SQL concurrency.

384 varied error/read sequences use seed 0x472d2026; 128 reachable check
sequences use 0x472e2026. Mandatory tests include nonempty plans/results/prior
reports/findings, every optional field, all gate outcomes, errors, bounds and
Unicode. Persisted regressions live beside the test. A read tape advances on
every read, never by key, so caching/duplicating a read changes the trace.

One unrelated pre-existing behavior remains: a nonempty fallback adoption plan contains
goal_ids from maker_inputs and fails Check's deny_unknown_fields decode.
The fallback retains its deterministic shell witness. #838 separately checks the old malformed-findings panic and the new boundary error; only structurally valid records participate in equality schedules. The new property runs 256 changing-read schedules with every write crash point and compares serialized record bytes.
