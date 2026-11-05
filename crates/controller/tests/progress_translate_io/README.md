# Progress and Quota translate conformance

G2d found no decision residue in either translate arm on
`cf674f1c4e6d5f807d2f03ca3f073c1a7a18b664`. Each arm passes its fields to an
allowance method and returns `Ok(None)`. Production stays unchanged. There is
no `translate_progress` API or new Verus group. D2 already owns and proves the
allowance decisions in `controller-core::looks_state`.

`reference.rs` and `current.rs` compile the real arm bodies separately. They also
compile the real `progress_allowance`, `quota_allowance`, `retire_wait`,
`allowance`, and `now_ms` bodies separately. `source.rs` checks their bytes against
SHA-256-pinned base fragments and current production. `types.rs` is the unchanged
production allowance state. `sync.py` refreshes these copies from Git and source;
it is a test-only source copier, with no product decisions.

The recorded leaf is the clock. Every sample consumes a separate tape entry.
The production RowChange type and a recording Commit row sink capture each
buffered push at its real call site, including its position relative to clocks. The outer
transaction boundary is modeled as one atomic write of serialized rows. Every
attempted commit, including failures, is replayed with a crash immediately before
and after it. Compare results, clock/call traces, buffered rows and persisted
bytes. This models transaction atomicity; it does not run PostgreSQL or the
translate prologue, tail, or filesystem subscription removal after commit.

Mandatory tests execute both arms, absent and malformed allowance rows, empty
and present checkpoints, unchanged progress, declared external waits, all model
option shapes, missing/equal/older/newer/malformed model reset rows, exact quota
replay, changed model/reset, distinct clocks, write errors, sequences and crash
atomicity. These assertions check concrete results and traces, not just parity.
256 generated schedules use fixed seed `0x00d2_0475_2026`. Committed regression
seeds are copied into writable `TEST_UNDECLARED_OUTPUTS_DIR` or `TEST_TMPDIR`;
proptest appends minimized failures there, never to the read-only source tree.

Citations were checked on the base. State-table:277 and state-machine:505 define
Q53 quota fallback; state-table:796,800 and state-machine:446,494 define allowance
and Q43 wait context. State-machine:507 and issue #759 describe the unserved
ladder bound. Exact replay, row ordering and wait-retirement details preserve
existing behavior where no rule specifies them. No new proof claim is made.

Run `//crates/controller:test_test_progress_translate_io` alone on BuildBuddy.
