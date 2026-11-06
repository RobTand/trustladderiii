# BuildBuddy operator client

`bbrun.py` is the canonical standalone operator client. Its initial baseline was
copied byte-for-byte from the approved shared client at
`/home/rob/tmp/bazel-wave2/tools/bbrun.py`, SHA-256
`f17ce7ae8de16a8e35e48aba3b5020daa00d60f55500e66788b41b730b0bc8e5`.
The only behavior added to that baseline is guarded cancellation and its two
service RPC routes. The existing offline `--selftest` remains unchanged.

Found by: operator, stuck or superseded BuildBuddy work. This is tooling only.

## Cancellation

```text
python3 -I bbrun.py cancel INVOCATION_UUID \
  --expect-commit FULL_LOWERCASE_SOURCE_SHA \
  --expect-repo EXACT_HTTPS_REPOSITORY_URL \
  [--protect-invocation OTHER_UUID ...]
```

The invocation must be a full lowercase nonzero UUID. Source and repository
guards are mandatory and must each appear exactly once. Repository matching
is literal, including case, trailing slash and `.git`. URLs with credentials,
ports, whitespace, query strings or fragments are refused. Protected IDs are
additive; the built-in protected invocation cannot be overridden.

The client reads the invocation's own ID, source SHA and repository URL, then
queries executions using that exact invocation ID. Exactly one execution with
a known stage is required. Missing, conflicting, paginated or malformed evidence
fails closed. The client rechecks source, repository, execution identity and
reported ownership before cancellation. A terminal invocation or execution is
a no-op. A disconnected invocation with a live execution is terminal with an
unknown result, also a no-op.

There are at most seven RPCs, each with a 15-second timeout. The client sends
`CancelExecutions` at most once and makes one follow-up observation without
polling. It never submits a build or signals a host process. Service errors and
arbitrary remote diagnostic strings are excluded from output. Evidence is JSON
lines with `before`, `ack`, `after` and `terminal` phases when those observations
are available. The existing credential evidence guard applies to each line.

The terminal outcome distinguishes `cancelled`, `already_terminal_success`,
`already_terminal_failure`, `terminal_unknown`, `unconfirmed` and `refused`.
Cancellation is confirmed only by a completed execution with gRPC status
CANCELLED, code 1. An acknowledgement alone is unconfirmed. Exit 0 means an
observed terminal cancellation, success or failure; exit 1 means refusal before
mutation; exit 2 means unconfirmed completion, unknown terminal result or invalid
CLI arguments. Invalid CLI arguments are rejected before credential loading.

The API has no conditional cancel operation. Ownership can change between the
last read and the mutation. The immediate recheck narrows that race; it cannot
eliminate it. A single follow-up read can return unconfirmed while cancellation
is still propagating. Inspect that invocation before deciding on another action.

## Installation and verification

Installation replaces only the shared `bbrun.py` after ops review and captain's
tooling fast-forward. The installed file uses the Python standard library and
never imports from a worktree. Existing submission modes still require sibling
`check_no_secret_leak.py`, SHA-256
`decd9ce671299ece1b245811af459349ed9932ee9b81b2c4c7a13cf96b94f609`.
Both the tracked scanner and the shared scanner matched this pin at handback.
No scanner replacement is required.

The handback includes the exact baseline-to-candidate patch, both file hashes,
and an installation script that refuses unexpected current or candidate bytes.
Rollback likewise requires the installed hash and restores the baseline bytes.
Those artifacts live under `/home/rob/tmp/tl3-build/ops/bbrun-cancel/`.
Review and shared installation remain ops/captain gates.

Run `test_bbrun_cancel.py` and `bbrun.py --selftest` through PrismaBuild or
approved free remote compute. The cancel tests drive the real CLI, request
serialization, HTTP routes and response parsing against fake HTTP responses.
They never use real credentials or send requests to BuildBuddy.
