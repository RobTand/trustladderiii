# Memory retain/curate extraction equivalence

The immutable base is f5b0ba2fb1. `base-memory.rs.txt` is the real Git blob,
SHA-256 8efab26226bc1c23c33ff1309608db45f8f5f99522852dcf0312e6df15e95502.
`sync.py` refreshes three separate, verbatim function bodies on each side:
`queue_attempt`, `flush`, and `curate`. The Rust source check pins each reference
body to the base, each current body to production, and forbids sharing any of
these changed functions. Every shared helper is source checked and unchanged.
The same check rejects edits elsewhere in memory.rs, including feed ownership,
fences, listener, recall, and incident/probe logic.

The recorder supplies per-call file bytes/errors, store reads/errors, write
failures including committed-but-lost responses, runtime/key/client failures,
and Hindsight success, key rejection, other 4xx, 5xx, timeout, landed-but-lost
response, duplicate, malformed body and each invalid acknowledgement field.
The unchanged file-read, redaction, UUID and evidence-checking implementations
execute against this boundary. `memory_attempt` is its real unchanged body;
its unchanged incident lookup is a held/not-held boundary projection. These
are controller-to-store call traces. The store shim models transaction outcomes,
not Postgres internals, and the existing real Postgres/feed targets remain.

Each schedule compares exact results, ordered calls and responses, upstream
effects, remote rows, persisted store rows and evidence file bytes. Each
attempted store write is rerun with a crash immediately before and after its
transaction effect. There is no new production timing or polling. The fake
remote deduplicates POST operations by their actual operation UUID. This is an
explicit remote contract; the pure proof does not claim to prove server code.

Three entries run 96 generated schedules each, plus a 92-case generated HTTP
outcome matrix that forces every outcome before a generated response suffix, with fixed seed 0x18_0776_2026,
plus the committed replay seed. Failures and minimized seeds persist in the
writable TEST_UNDECLARED_OUTPUTS_DIR, never the read-only source tree. Mandatory
cases cover every changed decision, nonempty flags/conventions, absent and
present evidence, each HTTP outcome, mixed per-call outcomes, lineage holds,
capacity replay, empty wakes, acknowledged deletes, before/after crash rows,
65-row page traversal and one-item HTTP batches. Explicit expected rows, traces
and effects supplement equality between the two versions.

Run `//crates/controller:test_test_memory_retain_io` alone, uncached on BuildBuddy.
The Verus contracts are in the real production core. The rerunnable mutation
lever is `crates/controller-core/verus/seed_memory_retain_mutant.py`.

The byte limit and HTTP/UUID predicates preserve existing behavior with no
state-table rule. #422 describes retaining useful memories, #403 specifies
postmortems, #776 records event-driven delivery holds, and docs/stack.md:24,56
on the base describe retention and outage semantics. None specifies the exact
HTTP table or proves remote deduplication. Those limits are stated explicitly.
