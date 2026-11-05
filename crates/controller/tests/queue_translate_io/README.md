G1 copies the Queue arm verbatim from cdacce8f99c28ca8dba8f5f76831b77b4dfcd836.
The reference arm is SHA-256 pinned. The current arm is checked against lib.rs.
Both compile under a fake controller that records check_run reads, intern INSERTs,
row pushes, incident-helper calls, notice effects and the transaction commit.
Every check_run call consumes its own response. Intern failures return normally.
Each schedule is replayed before and after every intern INSERT. The comparison
includes the call trace, buffered rows/effects, transaction-local intern dictionary,
persisted dictionary and committed rows. A crash rolls back the input transaction.

The common boundary compiles the unchanged QueueFact methods and incident helper
bodies. setup_incident is the shared sink, since group D owns its implementation.
CI sanitization helpers are copied unchanged. Redaction is an identity shim for
these synthetic inputs; this test makes no claim about redaction or notice text.
It checks the arm's calls into those unchanged helpers, including their arguments.

Run //crates/controller:test_test_queue_translate_io alone on BuildBuddy.
256 fixed-seed schedules use 0x004717602026; 128 event sequences use
0x004717602027. Both replay persisted regression seeds.
mandatory_* cases assert route reachability and concrete outcomes.

An additional 256 generated shape schedules use 0x004717602028. Mandatory shell
cases cover empty/nonempty CI checks, tests, permissions, queue conditions and
plan entries; app IDs, changed paths, plans and previous SHA presence/absence;
malformed bindings/incidents/plans, invalid SHAs and intern errors. Helper copies
are compared to both the pinned old blobs and current production source. The
only changed production shell body, Queue translation, is compiled separately
on each side; new pure decisions are exercised only by the current body and
verified directly in controller-core. No changed function is a shared shim.
