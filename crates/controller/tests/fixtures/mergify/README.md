# Queue simulation fixtures

`tests/mergify.rs` replays the recorded responses in `crates/mergify/tests/fixtures`
and the webhook envelopes in `crates/github/tests/fixtures`. The local REST server
substitutes the simulation's repository, PR number and head. It changes the queue
state after the label write. It never calls an upstream service.

The merge commit's successful CI check, the timeout conclusion, label removal,
merge group and draft PR variants are synthetic. They exercise fields absent
from the recordings. They are not live sandbox evidence.
