# Publication extraction I/O equivalence

The reference bodies and SHA-256-pinned base blobs come from
`cdacce8f99c28ca8dba8f5f76831b77b4dfcd836`. Source assertions tie whole reference
functions to those blobs and whole current functions to production. `sync.py`
refreshes copies and refuses changed frozen blobs. Keep the reference until the
extraction ships, then remove it in a follow-up.

Both shells execute against one recorder. It runs the production atomic writer
against a mock filesystem, including temporary bytes, file fsync, rename and
directory fsync. The epoch fence is a file in that same filesystem. GitHub reads
can change on every call. GitHub writes can succeed, refuse, hit rate limits, or
land and lose their response. The real comment helpers perform duplicate DELETEs
and POST preflight reads. The production approval-convergence loop PATCHes every
own-app copy. Writer invocations are recorded at the model boundary. Body and
obligation validators are production helpers, checked against their sources.
Branch formatting uses a fixed valid topic branch, and source-checked validators
still run. No clock is sampled by these entry points.

Four entries cover reconcile, publish, execute_delivery and fenced repair_approval.
The fixed seed is `0x0769_0730_2026`, with persisted regressions. Each entry and
scenario runs 64 schedules, drawing stable tapes, a changed read after a stable
prefix, and independent responses. Every attempted file or upstream write has a
crash cut before it and after its effect. Compare the full trace, result, upstream
state, upstream effects and every file's bytes.

The mandatory tests reach pr_evidence permanence, three-copy PATCH convergence,
stale-epoch refusal, Uncertain after a landed POST loses its response, clarification
fallback after message rejection, other-message permanent rejection, duplicate
comment deletion, close-reason closure, refusals and rate limits. They also pin the
two Refused writes for a 404 reported as unsent. No extraction changes that behavior.

Run `//crates/controller:test_test_publication_io` alone on BuildBuddy. The existing
approval-race and publication adapter scenarios remain unchanged.
