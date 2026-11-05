# Worker translate I/O equivalence

This seam compiles the WorkerFailed and WorkerReply arms verbatim from base
`6ba8d25035` and from current production. `base-arms.rs.txt` is SHA-256 pinned;
the source guard compares both copies against their authoritative source.
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

Reviewer verdict and head presence are generated independently. The successful
findings property varies nonempty blocker counts/text, existing feedback and
material/document tapes, and asserts a committed populated maker_findings row.
It feeds those exact rows into a maker reply and replays both sides of its intern
write. Mandatory shell witnesses assert RunFailed and the expected call/row
trace for invalid maker paths and known request-scope citations without a quote.

#828 retains the current WorkerReply arm verbatim after ordering quota success
evidence before ingress. These differential schedules contain legacy quota rows
without ending timestamps, so their original unconditional consumption and the
new legacy-only consumption agree. `quota_hold` covers ordered quota rows and
successes through real immutable worker endings, including delayed success and
equal reset hints. No reference arm, generated schedule or assertion is removed.
