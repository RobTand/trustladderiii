# Reviewer input extraction equivalence

The frozen base is f5b0ba2fb16a686232c08fe5cee907f1d43c5258.
`base-review.rs.txt` is its complete blob, SHA-256 pinned in `pins.json`.
`reference.rs` compiles its complete real `inputs` body. `current.rs` compiles
production's complete real body, including the new core calls. Source checks
reject drift. `sync.py` updates only current, never the frozen base or reference.
No changed function routes through a shared implementation.

Store and Git leaves use one recorder with independent response tapes for every
call. In particular, three document reads and three tree reads can each return
a different value or error. Compare the complete ordered trace, arguments,
responses, result/error and serialized input bytes. There are no writes in
`inputs`, so no durable state or write-crash boundaries are modeled.

Unchanged criteria/goals/clipping leaves retain production bodies. Validation
is a recorded leaf; this test measures the exact value reaching validation and
its failure boundary. The real validator and real store/git adapters run in the
retained controller worker targets. No claim is made that synthetic receipts
are schema-valid. #717, #704/#706/#719/#723 motivate opaque wire preservation;
they do not define new caps or authorize summary rewrites here.

Generated schedules use seed 0x1707302026 and 112 cases plus the committed
minimized extraction-stub seed. Each run copies the seed file to writable test
output before replay; failures persist there, with the full minimized schedule.
Mandatory tests assert exact expected serialized inputs, read sequences and
arguments, each error boundary, both diff limits, Unicode scalar caps, every
base route, optional issue/fix/receipt shapes, independent replies/trees,
selection order, duplicate preservation, and empty/20/21/101-entry collections.
There is no reviewer-input collection count cap; the neighboring findings cap
of 20 must not accidentally appear here.

Run `//crates/controller:test_test_review_inputs_io` alone, uncached, on
BuildBuddy. This harness is temporary until the extraction ships.
