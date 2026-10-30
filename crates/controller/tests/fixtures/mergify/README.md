# Queue simulation fixtures

`tests/mergify.rs` replays the recorded responses in `crates/mergify/tests/fixtures`
and the webhook envelopes in `crates/github/tests/fixtures`. The local REST server
substitutes the simulation's repository, PR number and head. It changes the queue
state after the label write. It never calls an upstream service.

The default merge commit has no checks. Queue merges read the recorded Mergify
merged check on the PR head. Outside-merge CI checks, timeout conclusions,
label removal, merge groups and draft PR variants are synthetic. They exercise fields absent
from the recordings. They are not live sandbox evidence.
