# Looks I/O equivalence

The reference is the complete cdacce8f looks source blob, SHA-256 pinned by
`source_pins_and_production_copies`. `reference.rs` compiles verbatim methods
from that blob. `current.rs` compiles verbatim production methods. Run
`python3 crates/controller/tests/looks_io/sync.py` to refresh copies after a
production edit. The assertions reject drift on both sides.

The smallest seam is a `controller!()` macro that supplies Controller's fields
and the methods outside the extracted slice. Both versions use the same production Delivery,
Look, Completion, store Snapshot, Commit and RowChange types. The original
AllowanceState and incident serde boundaries compile unchanged. The full
look_state, look_failed, look_alive, finish_stall, rearm, probe_passed,
look_observations, save_allowance, allowance_row and record_progress bodies
run against one thread-local recorder. Neither observe nor queue_observation
is extracted or edited: the recorder supplies their boundary calls.

Store loads and adapter calls consume independent tape frames, including
unrecoverable stored-data/store errors, quota, permission, transport, changed
reads, head changes and GitHub 404/500. A load can return a changed incident
without changing the next load. Clock samples consume their own frames.
Calls and results, mutable returned state, complete commit payloads, publication
records and every persisted row/effect byte are compared. Every attempted
write, including a failed write, repeats with a crash immediately before and
after the mutation. Store row changes are staged together and become durable
at the transaction commit boundary.

The shared shell shims cover unrelated boundaries, not alternate decision
implementations. Scenarios keep requests active with no worker-input context or
queue setup evidence. The request projection is fixed;
publication diagnostic presence varies; worker-input prose is absent. Kernel
observe/complete calls, notice retry, hold changes and ladder notes are recorded.
The retained real-controller looks scenarios cover those boundaries separately.

128 proptest schedules use seed `0x00d1_0760_2026`; the committed regression file
is replayed. Mandatory tests assert actual branch effects for first/second
failure, a later incident, first probe clearance, Rob-only later incidents,
silence-period deduplication, maintenance, postmortems, incident notices and
refused-command hold. Each comparison includes every reachable crash cut.

Run `//crates/controller:test_test_looks_io` alone on BuildBuddy.


D1 r1 extends the recorder beyond the original restricted projection. Completion
observations vary between empty and a kernel fact, and incident rows vary between
an effect incident and a kernel incident with no effect. `observe` persists the
observation identity and payload, runs the real `tl3_kernel::step` for kernel
observations, and records the resulting node incident state and kernel outputs.
The kernel itself is unchanged. Queued fixture nodes let the mandatory tests
assert both the first stall transition and the already-waiting guard. The
Rob-authorized `clear_incident` route remains covered by the real-controller
scenario because lib.rs is owned by group G. A second allowance input exercises
per-effect counter isolation. Every new recorded write participates in the same
before/after crash sweep. No verbatim-body name list changes were needed for r1.

D2 adds the verbatim `progress_allowance`, `quota_allowance`, and
`expire_model_quotas` bodies to sync.py's METHODS list. The expiry helper is
unchanged production code called by look_state; compiling it avoids a shared
shim that could hide quota-store reads or commits. `look_kind` and `wait_status`
are the new current-only typed converters and are checked against production.
The base digest remains unchanged.

The D2 schedules cover every Look variant, GitHub not-found recovery, pending
and settled effects, within and beyond allowance, stopping, quota resume,
ladder-out notice boundaries, external wait owner records and errors, progress
retirement, and quota replay by model. Four completion shapes cover empty,
progress token only, observations only, and both. Complete, ladder note, watch,
and observe effects are persisted and included in every crash comparison.
D2 runs 128 additional schedules with seed `0x00d2_0760_2026`, plus six mandatory
branch tests. The repeated-look witness checks the notice is emitted once and
its allowance flag persists. D1's mandatory tests and 128 schedules remain.
