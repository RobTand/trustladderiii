# D1 looks I/O equivalence

The reference is the complete cdacce8f looks source blob, SHA-256 pinned by
`source_pins_and_production_copies`. `reference.rs` compiles verbatim methods
from that blob. `current.rs` compiles verbatim production methods. Run
`python3 crates/controller/tests/looks_io/sync.py` to refresh copies after a
production edit. The assertions reject drift on both sides.

The smallest seam is a `controller!()` macro that supplies Controller's fields
and the methods outside D1. Both versions use the same production Delivery,
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
implementations. Scenarios keep requests active with no worker-input context,
queue setup evidence or kernel-owned incident. The request projection is fixed;
publication diagnostic presence varies; worker-input prose is absent. Kernel
observe/complete calls, notice retry, hold changes and ladder notes are recorded.
The retained real-controller looks scenarios cover those boundaries separately.

128 proptest schedules use seed `0x00d1_0760_2026`; the committed regression file
is replayed. Mandatory tests assert actual branch effects for first/second
failure, a later incident, first probe clearance, Rob-only later incidents,
silence-period deduplication, maintenance, postmortems, incident notices and
refused-command hold. Each comparison includes every reachable crash cut.

Run `//crates/controller:test_test_looks_io` alone on BuildBuddy.
