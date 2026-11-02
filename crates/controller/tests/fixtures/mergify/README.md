# Queue simulation fixtures

`tests/mergify.rs` replays the recorded responses in `crates/mergify/tests/fixtures`
and the webhook envelopes in `crates/github/tests/fixtures`. The local REST server
substitutes the simulation's repository, PR number and head. It changes the queue
state after the label write. It never calls an upstream service.

The default merge commit has no checks. Queue merges read the recorded Mergify
merged check on the PR head. Outside-merge CI checks, timeout conclusions,
label removal, merge groups and draft PR variants are synthetic. They exercise fields absent
from the recordings. They are not live sandbox evidence.

`pb2625/` records prismabuild#2660's queue failure on draft #2661 on
2026-11-02. `queue-check.json` is the Mergify check on the original PR.
`gate-result.json` retains the changed paths, failed labels and commit IDs
from the redacted Gate artifact. `gate.log` retains its failing-test line.
The local server supplies the draft check and log endpoints without live calls.
Sources: https://github.com/RobTand/prismabuild/pull/2660,
https://github.com/RobTand/prismabuild/pull/2661 and
https://github.com/RobTand/prismabuild/issues/2662.
