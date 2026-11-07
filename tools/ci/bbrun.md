# BuildBuddy operator client

`bbrun.py` is the canonical standalone operator client. Its initial baseline was
copied byte-for-byte from the approved shared client at
`/home/rob/tmp/bazel-wave2/tools/bbrun.py`, SHA-256
`f17ce7ae8de16a8e35e48aba3b5020daa00d60f55500e66788b41b730b0bc8e5`.
Additions to that baseline include guarded cancellation, its two service RPC
routes and safe HTTP diagnostics. The existing offline `--selftest` remains unchanged.

Found by: operator, stuck or superseded BuildBuddy work. This is tooling only.

## HTTP diagnostics

HTTP failures retain the numeric status, up to 512 characters of reason and
up to 1024 characters of body excerpt. The client reads at most 4096 body bytes
and limits the complete diagnostic to 2048 characters. It replaces the known
API key with `[key]` before truncating diagnostic text. URL failures retain
their scrubbed reason. A failed body read still reports status and reason.
There are no automatic retries or request/header dumps. Timeout and malformed
JSON diagnostics retain their existing behavior. Raw/gzip evidence still
refuses credential-bearing data rather than replacing it.

Found by: RP Kani coordinator setup failure, reported only as `HTTPError`.
The historical response is unavailable; this change cannot recover it.

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

Scanner rejections retain safe diagnostics in runner stdout as
`BBRUN_SCAN_DIAGNOSTICS_V1` JSON lines, before the final wrapper-result line.
The same records are written to private runner state as
`scan-diagnostics.v1.json`. The rejected upload stays empty; the private file is
not an automatic upload artifact. The runner log is the durable evidence when
artifact bytes are discarded. Wrapper-result v1 fields and scanner exits are
unchanged.

Each diagnostic has schema `tl3.bbrun.scan-diagnostics`, version `1`, a phase
(`initial`, `remaining`, `receipt`, or `unobserved`), `scanner_exit_code`,
`diagnostics_status`, `detections`, and `incomplete_report_count`. Each detection
has a safe upload-relative `path`, `rule_id` from the scanner's environment
labels, and `match_count: 1`. This counts a reported rule detection per file per
scan, including compressed members. Repeated raw or encoded occurrences within
a file still count once. Counts from different scan phases must not be summed
as unique files or byte occurrences. `incomplete_report_count` counts recognized
incomplete-file reports, not matches.

Only exact scanner reports corresponding to the current file inventory are
recognized. Unknown, empty, duplicate, ambiguous, or unparseable reports make
`diagnostics_status` partial. Scanner exceptions produce partial evidence and
missing scans produce unavailable evidence. These statuses describe diagnostic
coverage; they never authorize upload or replace the scanner verdict. A complete
diagnostic record can accompany an incomplete scanner verdict such as exit 75.

Credential forms are removed from paths using the pinned scanner's redaction
forms. Paths with colons, non-ASCII/control characters, or more than 512
characters are withheld as JSON null, as are private receipt paths. Filenames
with line separators make the entire scan's interpretation partial with no
attributed detections, because they can forge line framing. Redaction collisions
also prevent attribution. At most 32 detections are retained per scan, with
partial coverage if more were reported. Paths are never truncated. A final
credential-form check can withhold a record. Raw scanner output, exception text,
matched bytes, and surrounding content are never emitted. No scanner source or
rule changes are involved.
