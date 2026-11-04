# Enqueue extraction I/O equivalence

The reference is pinned to pre-extraction commit
`e3292d911a6e42c89f1f197b90648419a39a6b3f`. The `base-*.rs.txt` files are
verbatim Git blobs. SHA-256 assertions pin their bytes. The reference modules
compile whole function bodies copied from those files. The current modules
compile whole production function bodies. Source assertions reject drift in
either set of functions. Keep these copies until the extraction merges, then
remove them in a follow-up.

Both versions use one recorded boundary in `io.rs`. It supplies side-file bytes,
metadata, store reads, clock values and GitHub responses. The unchanged atomic
record writer executes against that boundary, including temporary files, file
fsync, rename and directory fsync. The real comment helpers perform duplicate
DELETEs and the command POST preflight read against recorded leaf operations.
Body healing, issue reads, PR-body PATCHes, workflow reads/reruns and queue-fact
requirements, classic statuses, annotations, suites, job logs and changed paths
are recorded too. Snapshot projections and branch ownership remain pure shims.
Publication validation has no obligations in these schedules; its shared shim
asserts that boundary. The retained real-adapter scenarios cover that behavior.

Seven generated entry/profile pairs run 128 cases each. A separate property
runs 128 recovery-file appearance cases. Config fixes the RNG seed to
`0x7517602026`, and loads `proptest-regressions.txt` for persisted seed replay.
That file also pins an additional seed derived from the exact JSON regression.
The strategies and dependency lockfile are committed, so Gate runs the same
cases. A failure saves its seed for reproduction and minimization.

Tapes include stable paths, one changed read after up to 127 stable reads, and
up to 127 independently changing read responses. Leaf overrides target named
reads without depending on how many earlier operations are recorded. Every read
can change on the next call. File reads include NotFound, PermissionDenied,
Other and invalid JSON. File writes can fail normally with PermissionDenied,
StorageFull or Other. Upstream writes include success, refusal, rate limiting,
and transport failure with and without a landed request. Mandatory cases assert
that mergeable=false, active/terminal queues, refusal replies, classic statuses,
body healing, queue evidence, duplicate DELETEs and command preflight are reached.

For each schedule, compare the full I/O trace and results, returned
completion/error, upstream effects and every file's bytes. Then repeat
immediately before and after every attempted file/API write, including failed
writes. Crashes stop execution immediately; expected crash panics are caught,
while unexpected panics fail the test.

`waiting-budget-disappears.json` preserves the batch-five schedule verbatim.
The recorder now observes more earlier I/O, so the companion
`waiting_timer_reread_does_not_revoke_grace` targets the same recovery reread by
leaf name. It asserts that dispatch is reached when a timer disappears after
grace was already authorized. The other fixed case retains the earlier
recovery-file appearance between two distinct reads.

Run `//crates/controller:test_test_mergify_io` alone on BuildBuddy. Its ordinary
Rust test runner reports nonzero test counts and comparison counts. Keep the
whole shard below 60 seconds. It complements the original 183 adapter scenarios
and Kani proofs; all scenario groups remain KEEP.
