# Verus measurement driver

`submit.py` prepares and submits one exact-source measurement. Its full dry-run
is the preparation gate. `fixture-check.py` checks that gate without running
Bazel, Verus, or the real scanner. Development checks run through PrismaBuild
or supported free remote compute.

## Configuration

`normal.example.json` describes one selection, not an authorized proof run.
Every configuration field is required. `head` and `comparison_base` are exact
SHA40 labels. The source checkout must be clean and at `head`, including
untracked files. The base object and parent history are unnecessary. A base label
alone does not establish a measured comparison.

The selection manifest has unique targets and unique selectors with positive
integer counts. The current adapter supports entries in the kernel `groups`
table in `tools/bazel/verus.bzl`. It validates each tuple against the exact
source and checks the target-generation anchors. Other inventories need a
separate explicit adapter.

`frontend-only` and `normal` require a 45-second inner watchdog and a 180-second Bazel timeout.
`diagnostic` requires 120 and 180 seconds, plus a nonempty
`diagnostic_authorization` reference. The reference records separate approval;
it does not grant approval. The solver budget remains 30, with one verifier
thread, a five-second kill margin, and the existing solver path and environment
pins. Elapsed time above the historical 20-second ceiling produces a warning.

`raw_queries=false` collects ordinary artifacts. Explicit `true` adds query
logging and stages unpacked query files with the same file checks. It does not
fall back to copying a complete output tree. Archived raw queries are currently
unsupported and fail collection.

## Preparation and submission

The following command runs inside an admitted action or supported remote job.
Paths are examples. Keep `--output` outside the clean source checkout and use a
new directory for each attempt.

```bash
python3 -I /path/to/tooling/tools/ci/verus_measurement/submit.py \
  --config /path/to/config.json \
  --source /path/to/clean-shallow-source \
  --output /path/to/evidence/preparation-unique \
  --receipt /path/to/evidence/receipt.json \
  --runner-log /path/to/evidence/runner.log \
  --dry-run
```

Preparation compiles both Python files and the exact compressed remote command,
hashes every tracked source file, validates selections and the committed runner
capability, and stages runner bytes, an empty compatibility patch, Bazel argv
and submission argv. It stops before Bazel or BB submission. Source cleanliness
is checked before output creation. No mode patches runner source. Finalization
checks every tracked source byte against the staged identity and records an
explicit restoration outcome, including failed collection.

The receipt binds both scripts, source, config, selection, instrumentation, and
exact submission argv, runner bytes, pins and mode. Executed tooling may differ
from measured tooling only through the explicit hashed `tooling.json` provenance.
Every dry-run invalidates the old receipt before config
parsing. Successful completion replaces it atomically. Reusing an evidence
directory, changing an argument, or failing preparation leaves no usable prior
success. Submission reports the differing receipt fields when it refuses stale
evidence.

To submit an independently authorized frontend check, set `mode` to
`frontend-only`, keep `raw_queries=false`, omit `--dry-run` and choose
a new submission output directory. Keep the config, source, receipt, and runner
log arguments identical. Submission checks BB-HOLD immediately before its one
BB submission. Exit 2 resumes only with the client's exact own-invocation
`still running` marker. Waits are bounded to the coordinator duration rounded
up in ten-minute waits; exhaustion records UNKNOWN and never resubmits.

For normal or diagnostic work, pass `--frontend-receipt` pointing to the own
frontend `terminal-attribution.json` during both dry-run and submission. The
frontend config must have the same exact head, comparison label and selections.
Submission independently rereads that own parent's terminal verdict, scanned
wrapper receipt and curated CAS bytes. A local PASS field is only a locator.
Missing, stale, failed or conflicting frontend evidence refuses submission.
The compact prerequisite ticket is bound into the exact submission argv. The
remote driver requires it before Bazel and checks the actual measured tool
hashes against the frontend tools. The terminal collector repeats these checks.

A source predating `tl3.verus-runner.modes.v1` is unsupported, including current
proof branches created before this tooling. Update that source explicitly to
reviewed/landed tooling, then prepare new exact-source receipts. Never inject a
new runner into an older candidate or treat an old SHA label as adoption.
Diagnostic configuration and raw-query collection remain supported through the
committed runtime mode, with the existing explicit authorization and 120s inner
watchdog. Older diagnostic sources also require an explicit source update.
Legacy ordinary targets and `--measure` retain their previous behavior unless
`TL3_VERUS_MODE` explicitly selects a modern mode; conflicting selections fail.

The exact BB command uses the approved `python3 -I .../bbrun.py` client. Terminal
inspection uses `python3 -I .../bb_logs.py`. Named CAS downloads use the approved
bbrun artifact operation and verify the URI's SHA256 and byte length.

## Evidence and verdicts

The driver stages exact source identities, selections, unchanged runner bytes and hash,
command, selected-child test.log and XML, and inventory, verifier, measurement
and execution JSON. Execution records actual argv, canonical tools and SHA256/
lengths, sysroot, pinned distribution manifest, environment, process status and mode. Every staged file is a new owned regular file with mode 0600 under a new
0700 directory. It rejects symlinks and special files without changing input
modes. Bazel's top-level testlogs link is the sole deliberate link boundary.
ZIP collection reads only named ordinary receipts and rejects duplicate or
special members. Ordinary mode never copies AIR or a complete test.outputs tree.

An omitted verifier `success` field is allowed only with positive exact count,
selection, zero-error, and error-flag checks. Explicit false fails. Missing
resources remain null and UNKNOWN; numeric zero remains zero. `rlimit_run_sum`
uses only `times-ms.smt.rlimit-run`, with init retained separately. Normal metrics
require `--time-expanded --output-json`, a nonempty function breakdown, and exact
agreement between its sum and the raw run count. Function count and maximum are
retained. `resources=PASS` means complete consistent metrics, never external
resource acceptance. That external gate remains <=30M OR
`200*head <= 201*exact_measured_base`; a base SHA alone provides no measurement.
No profiles run by default.

Frontend uses actual pinned `--no-verify` without `--build-sst`. It reports
`FRONTEND_ONLY`, `proof=NOT_RUN`, and `resources=NOT_APPLICABLE`. Success requires
exit 0, a clean zero-proof JSON verdict, zero worker threads, explicit zero SMT
counters and empty INIT/run query measurements. Positive expected obligations
remain source-selection metadata. Missing JSON, metrics, pins, tool identities,
source identities or no-SMT evidence cannot pass. No zero-query receipt can
satisfy normal positive-proof validation or resource acceptance.

Terminal collection checks the same own parent, verifies all curated CAS bytes,
rechecks preparation identities and raw child verdicts, and retains attribution
in `terminal-attribution.json`. Command, artifact handoff, scanner, cleanup,
wrapper, parent, driver collection, and source restoration remain separate.
A successful child or client exit cannot establish overall success. Exit 0 means
PASS, 1 means FAIL, and 3 means UNKNOWN. A known failed parent always means FAIL,
even without published driver artifacts. Unavailable evidence stays UNKNOWN;
retrieved contradictory evidence fails collection. Malformed terminal verdicts
are retained as UNKNOWN with attribution instead of bypassing its record.
Raw failed-child XML/logs are staged before validation, and collection continues
across selections. Returned group metrics and their sum are reconstructed from
verified raw receipts, excluding init, rather than accepted from the summary.

The consumer uses ops wrapper-result v1, `tl3.bbrun.wrapper-result`, version 1.
Ops owns `bbrun-wrapper-result.v1.json`, its authoritative schema, sanitized
reason vocabulary, scanner, and cleanup. This directory defines no competing
schema and installs no wrapper. The consumer rejects duplicate keys, unknown
versions, malformed sanitized reason codes, extra or missing fields, booleans as exit codes,
contradictory stages, and a mismatched parent ID. A nonzero first scan followed
by a genuinely clean final scan is allowed.
Qualifying PASS reasons and `no_cleanup_targets` are pinned exactly. Non-PASS
reason codes follow ops' schema pattern and are retained, including
`handoff_not_observed`, `cleanup_not_observed` and `receipt_scan_rejected`.

A receipt is read only after the own parent is independently terminal. Own CAS
attribution and driver namespace evidence are required. The remote command
checks that the wrapper's reserved name is absent before and after its work.
A fallback needs a separately proven wrapper-step stdout body, whose final
nonempty line is the unique result marker. A complete parent runner log also
contains command output and BuildBuddy footer lines and cannot prove that
boundary. The automatic collector currently has only that complete log, so
log-only evidence remains UNKNOWN. It never promotes an arbitrary earlier
marker into wrapper authority. A supplied receipt and any visible parent-log
marker must agree; repeated, embedded, or conflicting markers fail. Dedicated
body recovery is exercised with explicitly fake fixtures only. A missing v1
producer stays UNKNOWN.

`fixture-check.py` uses expressly fake wrapper verdicts and fake CAS responses.
They exercise the consumer, not the scanner. They never upgrade retained RP
results. Retained parent `d765f39c-35dd-4dcd-b013-3005b046c59c` remains FAILED,
and its initial scanner reason remains UNKNOWN. Real scanner regression belongs
to ops. Actual verifier qualification and captain merged-head tooling gates are
outside this producer fixture scope.

The precise embedded remote Python is executed through full pre-Bazel
preparation in the admitted fixture. Python 3.10 or later is required. Output
inside the clean source checkout is refused before directory creation. Focused runner compilation, fixtures and any authorized frontend smoke are
reported in the producer handoff. Actual normal verifier/scanner qualification
remains UNKNOWN until an independently authorized landed-head run.

The pinned wrapper's exact shell-quoted print-statement echo contains marker
text but no JSON verdict. Collection records this single source echo separately.
Changed/repeated source echoes and embedded/repeated emitted JSON results fail.
This distinction provides no fallback authority; own scanned receipt validation
and emitted result agreement remain mandatory.
