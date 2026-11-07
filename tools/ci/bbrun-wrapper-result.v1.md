# bbrun wrapper result v1

Owner: TL3 ops. Consumer: fv-port reusable measurement driver.
Status: read and accepted by fv-port before either builder started.
Baseline shared client: SHA256 89c8a850b9381cd2e4d52a2d94f45b5c2272c9fcb7a3708ef2a8c45552836763.
Found by: artifact cleanup, retained RP parent d765f39c-35dd-4dcd-b013-3005b046c59c.

## Receipt and ownership

The reserved artifact filename is `bbrun-wrapper-result.v1.json`. Its JSON Schema
is `bbrun-wrapper-result.v1.schema.json` beside this document. `schema` is
`tl3.bbrun.wrapper-result` and `version` is the integer `1`. The ops cleanup
slice will land the contract and schema under `tools/ci/` with the wrapper.
The filename is wrapper-owned. A command-created file at that name is a
collision and cannot become an authoritative receipt.

The shared wrapper creates the receipt. The measurement driver never creates,
edits, executes the scanner for, or supplies a missing wrapper receipt. It reads
the receipt only after its own parent is terminal, through the normal collector.
The collector retains the receipt bytes, SHA256 and own parent invocation/CAS
attribution. A supplied parent ID must match that parent. A null ID is allowed
when the runner has no trusted attribution; the collector must then establish
ownership from its own parent artifact or runner-log CAS. Never infer ownership
from a filename or another campaign's receipt.

## Fields and meanings

All fields in the schema are required, including explicit nulls. Duplicate keys,
unknown versions, malformed values and contradictory statuses fail receipt
validation. Missing receipt/status is UNKNOWN, never PASS.

| Field | Meaning |
| --- | --- |
| `parent_invocation_id` | Validated UUID from a trusted runner context, or null. Attribution only; no prediction of parent terminal status. |
| `command.exit_code` | Original subprocess return code, including a negative signal return, or null if unavailable/not started. |
| `command.status` | `pass` only for exit 0; `fail` for a known nonzero exit; otherwise `unknown`. |
| `artifact_handoff.status` | `pass`, `fail` or `unknown` for wrapper staging/handoff into the scanner. Independent of the driver's collection/staging manifest. |
| `scanner.status` | `pass`, `fail`, `error` or `unknown` for the complete artifact-scan decision. `pass` requires the pinned required-key scanner to authorize every remaining upload file. |
| `scanner.first_exit_code` | First actual scanner return code, or null if none was observed. Retained even if a permitted drop/rescan follows. |
| `scanner.exit_code` | Last actual scanner return code, or null if none was observed. An exception is `error`, not an invented scanner exit code. |
| `scanner.reason` | Stable sanitized reason code, never raw scanner diagnostics. |
| `cleanup.status` | `pass`, `not_needed`, `fail` or `unknown`. Covers every cleanup obligation owned by this wrapper, not the driver's source restoration. |
| `cleanup.reason` | Separate stable sanitized reason code. Never replaces scanner or command results. |
| `wrapper.original_exit_code` | Wrapper result derived from command/handoff/scanner before any cleanup failure changes the final result; null if not derivable. |
| `wrapper.exit_code` | Actual final wrapper process exit code, or null if finalization was interrupted. |

Each stage has a `reason` using the lower-case code pattern in the schema.
Reason codes come from a finite wrapper-owned mapping, not an exception message,
path, command, environment value or scanner output. `unknown` is explicit.
Suggested codes include `command_succeeded`, `command_failed`,
`command_not_observed`, `handoff_succeeded`, `handoff_failed`, `scan_clean`,
`scan_rejected`, `scan_incomplete`, `scanner_exception`, `scanner_not_run`,
`scan_reason_unknown`, `cleanup_complete`, `no_cleanup_targets`,
`cleanup_permission_denied` and `cleanup_io_error`. The implementation must
document its exact emitted set. A historical unknown scanner cause stays unknown.

The consumer pins reasons that can qualify a PASS: `command_succeeded`,
`handoff_succeeded`, `scan_clean`, and `cleanup_complete`, with the explicit
`no_cleanup_targets` case below. Schema-valid sanitized non-PASS reasons remain
non-PASS evidence even when they extend the suggested list, for example
`handoff_not_observed`, `cleanup_not_observed` or `receipt_scan_rejected`.
They must not cause a failed receipt to disappear or turn into a success.

`cleanup.not_needed` counts as successful ONLY with reason `no_cleanup_targets`
and `targets_present=false`. It means the wrapper positively established that
there was no target requiring deletion. It does not mean cleanup was skipped,
interrupted or delegated. `cleanup.pass` requires `targets_present=true` and
positive evidence that every required target is gone. `cleanup.fail` or
`cleanup.unknown`, a missing field, or either contradictory combination prevents
qualification. `targets_present=null` means presence was not established.

The scanner may pass while the command fails. Cleanup may pass after a scanner
failure. Neither combination repairs the earlier failure. Preserve the original
wrapper result and record a separate nonzero final exit on cleanup failure.
Do not invent a passing scanner from an empty directory after failed cleanup.

## Publication and failure handling

Write the receipt atomically from private wrapper state, outside command-owned
staging. Keep raw diagnostics private and never put credentials or unchecked
exception text in the receipt. Existing scanner/key scrubbing rules still apply.
No automatic upload may contain residual unscanned files after a scanner or
cleanup failure. A receipt is not an exemption from scanning uploaded artifacts.

Always attempt receipt/status finalization in a finally path, even after cleanup
fails. If safe artifact publication is impossible, emit one bounded structured
runner-log line prefixed `BBRUN_WRAPPER_RESULT_V1 ` with the same sanitized JSON.
The collector may recover that line from its own terminal parent's retained,
scrubbed runner log. This fallback does not authorize an unscanned upload or make
a failed scan pass. If both forms exist their objects must agree; disagreement
fails validation. If no durable form survives, the consumer records UNKNOWN.

The wrapper emits the receipt as its final stdout line before exit. This is not
a promise that it is the final nonempty line of the entire parent runner log:
BuildBuddy subsequently appends command-exit, upload, cleanup and completion
messages. A consumer demanding a literal final line of the complete parent log
must refuse the fallback when that condition is absent. Use an independently
established final wrapper-step output boundary if the transport provides one;
do not search arbitrary earlier output for a convenient receipt. The artifact
is preferred. Parent attribution/CAS binds transport, not cryptographic receipt
authenticity, and never overrides terminal parent failure.

The implementation must prove failure-safe publication as part of its scanner
and read-only-tree fixtures. It must not report `artifacts deleted` if deletion
failed. No process can promise a receipt after SIGKILL or unavailable storage;
those cases remain UNKNOWN and cannot qualify.

## Driver boundary and qualification

The driver owns preparation/staging manifest, exact child proof/count/resource
evidence, collection and restoration. It owns no scanner execution or credentials.
The collector reads terminal parent status independently through approved result
retrieval; the wrapper receipt has no parent PASS field.

Overall qualification requires all of the following on the same own submission:

- Driver preparation, source restoration, child proof/count and collection PASS.
- Wrapper command PASS with exit 0 and artifact handoff PASS.
- Wrapper scanner PASS.
- Wrapper cleanup PASS, or the explicit valid `not_needed` case above.
- Wrapper original and final exit codes both 0.
- Independently observed terminal parent PASS.

Missing, unknown, inconsistent or failed evidence in any required component
prevents PASS. Child success cannot override scanner, cleanup, wrapper or parent
failure. Measurement resource fields retain their own null/UNKNOWN semantics;
missing metrics are never numeric zero. This agreement authorizes no RP proof
run and does not reclassify retained failed parent evidence.

## Emitted reason codes

The implementation emits only the following stage reasons. Scanner diagnostics,
paths, exception messages and command text never supply a reason.

| Stage | Exact codes |
| --- | --- |
| Command | `command_succeeded`, `command_failed`, `command_not_observed` |
| Handoff | `handoff_succeeded`, `handoff_failed`, `handoff_not_observed` |
| Scanner | `scan_clean`, `scan_rejected`, `scan_incomplete`, `scan_reason_unknown`, `scanner_exception`, `scanner_not_run`, `receipt_scan_rejected` |
| Cleanup | `cleanup_complete`, `no_cleanup_targets`, `cleanup_permission_denied`, `cleanup_io_error`, `cleanup_not_observed` |

`scan_reason_unknown` preserves an observed scanner code when diagnostics cannot
be classified under the existing drop policy. `scanner_not_run` has null scanner
codes. `scanner_exception` preserves any earlier observed codes and never assigns
an exit code to the exception. The retained RP failure's initial scanner cause
remains UNKNOWN; this implementation does not classify that historical run.

## Implementation boundaries

Command state is written after the command returns, outside command artifacts,
into a private `0700` directory. The command's raw subprocess return code is
retained, including negative signal returns. Missing or invalid private state
fails qualification. The legacy numeric status also remains available privately.
No runner-provided attribution source is trusted by this implementation, so
`parent_invocation_id` is always null.

The wrapper rejects a command-created reserved receipt name. It generates the
receipt from its own state, scans its private bytes with the same pinned scanner
and required key, and publishes complete bytes atomically without replacing an
existing name. The receipt scan cannot change a rejected artifact scan into PASS.
A successful receipt-file scan leaves the last observed scanner code at zero.
Rejected scans use the structured runner-log fallback only. Publication failures
also use that fallback and return nonzero. The wrapper emits the same bounded
structured line on successful publication so both forms can be compared.

Cleanup includes removal of an empty staging root and permitted dropped files.
Normal staging therefore reports `cleanup_complete` with `targets_present=true`.
`no_cleanup_targets` requires positively observed absence of deletion obligations;
a missing cleanup observation never takes that path.

Before deleting rejected uploads, the wrapper renames their exact upload root
into a newly created private sibling directory and recreates an empty upload
root. A deletion failure leaves rejected bytes outside the runner's exact upload
directory. Those private quarantine bytes are not claimed deleted. Operators may
need to remove them later. A rejected symlink root is detached as a directory
entry, without opening or changing its target.

Deletion accepts only the known staging root or quarantine roots created by the
same support instance. Directory components are opened with `O_NOFOLLOW`;
cleanup uses the fd-based, symlink-safe `shutil.rmtree` implementation. The
permission callback adds owner write and search permission only to the owned
parent of a failed unlink/rmdir. A failed traversal can add owner read/search
permission to its owned directory and retry that subtree. Files and symlink
targets are never chmodded. Each failed operation has at most one retry.
Python 3.12+ uses `onexc`; the `onerror` adapter supports Python 3.11's API.
Unsupported fd-based deletion fails closed.

This is a wrapper for a completed command, not isolation from hostile surviving
processes with the same UID. Concurrent writers can replace files between a scan
and publication or revoke access to parent directories. The fixtures cover a
root replaced by a symlink during deletion; fd-based rmtree rejects it. They do
not prove safety against arbitrary concurrent namespace or mount changes. A
parent that becomes inaccessible before detachment can prevent containment;
that condition reports cleanup failure and cannot qualify. Unreadable directories
that cannot be safely opened for permission repair also fail cleanup. SIGKILL,
interrupted finalization and unavailable storage can prevent every durable form;
consumers must retain UNKNOWN in those cases.

The offline producer qualification runs with:

```sh
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --anywhere --cpus 1 --demand mem_gb=1 \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  --timeout-s 300 --wait-s 180 -- \
  /usr/bin/python3 tools/ci/test_bbrun_cleanup.py --qualify
```

It runs fixtures, existing cancellation/HTTP tests, client selftests and the
repository's Ruff rule selection. It does not submit BuildBuddy work or replay
proofs. The baseline RED fixture loads the pinned baseline source from Git and
checks its SHA256 before execution.
