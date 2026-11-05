# Post-match translate tail

The reference is the complete tail from f5b0ba2fb16a686232c08fe5cee907f1d43c5258.
`pins.txt` pins its exact bytes and the unchanged finding/report helper bodies.
The current copy is checked against production. Both compile against the same
recorded leaf I/O methods. No changed production body is a shared shim.

The boundary records worker-material/document reads, their results, diagnostics,
row pushes, and the final priority_reserve call. Independent read tapes include
absent values, malformed wire data and read errors. Each attempted row write is
replayed with crashes immediately before and after it. Results, exact traces,
stderr bytes, buffered rows and transaction-model persisted bytes must match.
This models transaction rollback, not PostgreSQL's on-disk format. Interning,
clock samples, upstream writes and filesystem writes do not occur in this tail.
The unchanged priority_reserve body is outside scope and is a recorded leaf.

Mandatory cases assert shell results and traces for every finding source and
absence, run/job/current generation admission, all checks booleans, detailed
review suppression, phase mismatches, infrastructure CI, malformed materials,
material/document errors, nested/flat goal precedence, duplicate and untrimmed
goal IDs, preexisting findings, first maker reply selection and Unicode clipping.
Generated inputs independently vary event kind, admission, wire shape and read
tapes. Runtime counters require every source plus absence and successful populated
findings. The fixed seed is 0x004732652026, 4096 schedules. Proptest replay seeds
are copied to a writable test-output directory before running and failures persist
there. The minimized red seed is committed in proptest-regressions.txt.
