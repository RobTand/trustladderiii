# Miscellaneous translation differential

The changed GitHubComment arm of Controller::translate is compiled verbatim in
reference.rs and current.rs against one recorded store and row-buffer boundary.
The reference arm is pinned to 9a658a8e3212b73fddca3b119ad17f25a86ad5bd;
the current arm is checked against production. The five assigned delegate-only
arms have no useful pure residue and remain unchanged, guarded against their
base fixtures. No changed production body uses a shared shim.

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
fixed seed 0x730_012c_2026, with persisted proptest-regressions.txt. Configured
repositories and snapshot rows can be empty or nonempty. This Comment type has
no optional evidence fields. Old and new use the actual public repository,
comment, error, snapshot, event and row-change types.

The seam is only this arm, matching G1's &mut self pattern. It excludes the
operations-comment prologue and shared post-match tail. The arm buffers rows,
so harness.commit explicitly models atomic outer commit/rollback and its bytes;
it does not exercise the real Postgres transaction writer, epoch fencing or
filesystem atomic writer. Those unchanged implementations are outside G2c.
