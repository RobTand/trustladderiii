# Miscellaneous translation differential

The operations preemption prologue and six assigned arms of Controller::translate compile verbatim in
reference.rs and current.rs against one recorded store and row-buffer boundary.
The reference arm is pinned to 9a658a8e3212b73fddca3b119ad17f25a86ad5bd;
the current arm is checked against production. The five delegate-only arms remain unchanged and are compiled on both sides
and source-checked against pinned base fixtures. Recorded unchanged leaf methods
vary operations preemption and delegate outcomes on each call. No changed production body uses a shared shim.

The real unchanged Store::comment_retired body is compiled in store.rs against a
recorded SQL client. Its exact source, SQL and parameters are pinned and checked
against production. Every SELECT consumes a fresh tape response, including database timeouts. The SQL leaf returns the real DatabaseError,
so the method and arm exercise both real error conversions. Tests compare calls, results, buffered RowChange values,
read/write counts and persisted bytes. Each attempted row push and explicit
harness commit is replayed with a crash before and after it.

Mandatory shell witnesses cover author and repository eligibility, case-insensitive
Repository membership, canonical identity, retirement, duplicate and irrelevant
snapshot rows, read failures, changing responses across repeated events, and
commit failure/retry. There are 256 generated event schedules of length 1..17,
fixed seed 0x730_012c_2026. Committed proptest-regressions.txt is copied into
TEST_UNDECLARED_OUTPUTS_DIR, falling back to TEST_TMPDIR, before each property
run. A persistence witness forces an expected failure and reloads its saved seed.
regression-schedules.json contains the concrete minimized original red schedule,
replayed through the same schedule builder as generated cases. Configured
repositories and snapshot rows can be empty or nonempty. This Comment type has
no optional evidence fields. Old and new use the actual public repository,
comment, error, snapshot, event and row-change types.

The seam uses G1's &mut self pattern and includes the operations preemption
prologue. It excludes the CI-evidence preparation, other arms, shared post-match
tail, and internal implementations of the unchanged delegates. The operations
and five delegation leaves are recorded sinks; tests exercise their calls and
returns, not their internal policy or database protocol. The arm buffers rows,
so harness.commit explicitly models atomic outer commit/rollback and its bytes;
it does not exercise the real Postgres transaction writer, epoch fencing or
filesystem atomic writer. Those unchanged implementations are outside G2c.
