# Worker translate I/O equivalence

This seam compiles the WorkerFailed and WorkerReply arms verbatim from base
`6ba8d25035` and from the historical current copy. `base-arms.rs.txt` is SHA-256
pinned. R2 removed the comparison of the moved production arms against that
copy. These arm comparisons are historical until R4a deletes the suite. The
controller feedback interface tests exercise the moved production writes.
The two copies share no changed production logic. Only the arms are changed
in `Controller::translate`; its prologue and common finding/feedback tail are
outside the seam.

Protocol mapping, reviewer criteria and validation, reproducing validation,
goal handover, findings serialization, evidence projection, quota clearing,
head interning and footprint construction compile real unchanged source bodies.
Their base fragments and current production fragments are checked on every run.
Three leaf methods have only their visibility widened in the test copy.

The recorded boundary supplies `worker_document`, `worker_materials`,
`check_run`, intern SELECT/INSERT, and row pushes. Protection and merge-dependency
preparation are unchanged delegate boundaries: the recorder captures their full
arguments, result and synthetic rows. Their internal algorithms are outside this
seam; the production worker and protected-request integration targets cover them.
No claim is made to test those algorithms with their recorded replacements.

Every read consumes its next tape entry independently. The maker reproduction
path deliberately retains both eager `check_run` calls even when the first plan
is present. Read results include missing values, malformed JSON and store errors.
Intern writes model an open transaction with separate pending and persisted
state. Every schedule replays a crash immediately before and after each attempted
INSERT, including failed writes. Compare call traces, events/errors, buffered
rows, pending intern state, persisted intern state, committed rows and world
footprint changes. This does not run the real DB transaction writer.

Fixed seeds are `0x7602b2026`, `0x7602b2027` and `0x7602b2028`, with persisted replay seeds.
Mandatory shell cases cover all admitted roles, rejected non-kernel roles,
stale generations, schema/role/criteria errors, findings exhaustion, maker
self-check success and each failure, eager plan reads, reproduction evidence,
WorkerFailed class independence, optional fields, nonempty collections and
rollback after buffering review evidence. The new pure step functions are proved
in the controller-core Verus targets. The pinned old arms provide the reference
for conformance of the shell's boundary projections and I/O schedule.

Run `//crates/controller:test_test_worker_translate_io` alone on BuildBuddy.

The current shell includes #833's typed prerequisite validation. Its resource
inspection calls the production Rust helper, and its buffered diagnostic write
is copied verbatim and source-checked. Existing differential schedules return
non-mismatching prerequisite results and preserve the original write/read order.
The real mismatch and rollback cases run in `test_test_input_resources`.

Reviewer verdict and head presence are generated independently. The successful
findings property varies nonempty blocker counts/text, existing feedback and
material/document tapes, and asserts a committed populated maker_findings row.
It feeds those exact rows into a maker reply and replays both sides of its intern
write. Mandatory shell witnesses assert RunFailed and the expected call/row
trace for invalid maker paths and known request-scope citations without a quote.

#832 adds the optional plan-check provenance field to the test observation. The
reference arm ignores exactly that field; its original arm bytes and all frozen
leaf hashes remain pinned. The historical current arm and its protocol mapping were copied
from production by `sync.py`, which runs on BuildBuddy. Only the current arm uses
`protocol-current.rs`; the reference keeps the original protocol implementation.
The differential schedules supply no provenance, so they check the dormant
fallback and preserve the existing I/O and failure order. Semantic-provenance
cases run through the real controller acceptance tests.

#828 keeps the reviewed historical WorkerReply arm from
683829169d577b921a1bc93e446b49399886a092 for extraction conformance. That historical copy includes the legacy model-only quota clear,
which is now forbidden in production. Its exact bytes and SHA256 are pinned separately in
legacy-quota-clear.rs.txt and pins.json. R2 removed the remaining arm comparison against production. Original differential schedules, I/O assertions and references stay
unchanged. The real-store quota_hold upgrade regression proves that an older or
other-family success preserves unknown legacy quota after restart.
