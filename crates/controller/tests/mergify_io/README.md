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
fsync, rename and directory fsync. The shared Mergify client methods execute
against the recorded GitHub client. Snapshot projections and the pre-existing
publication/body-healing/queue-fact reader are outside this extraction; the test
supplies their projections through the shared adapter reader and snapshot. The
retained integration scenarios cover those existing helpers with real adapters.

Each of five generated entry paths runs 256 schedules. A schedule varies initial
receipts, each read response and delay, and each upstream write result. Tape
strategies include stable paths, one changed read after a generated stable
prefix, and sequences of changing reads. Upstream write outcomes include
success, refusal, rate limiting, and transport failure with and without a landed
request. The focused recovery property separately generates which recovery read
first sees a file. Its minimal failing case before the fix was the second read:
old `Pending`, extracted `Absent`.

For each schedule, compare the full I/O trace, returned completion/error,
upstream effects and every file's bytes. Then repeat immediately before and
after every attempted file/API write, including unsuccessful writes. Compare
those traces, effects and bytes again. Crashes stop execution immediately;
expected crash panics are caught, while unexpected panics fail the test.

Run `//crates/controller:test_test_mergify_io` alone on BuildBuddy. Its ordinary
Rust test runner reports nonzero test counts and comparison counts. The 256-case
setting was measured below the 60-second shard budget. It does not replace the
183 original adapter scenarios or the Kani decision proofs.
