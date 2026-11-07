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

`normal` requires a 45-second inner watchdog and a 180-second Bazel timeout.
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

Preparation compiles both Python files and the exact embedded remote command,
hashes every tracked source file, validates selections and all runner anchors,
and renders the entire runner patch, preview, Bazel argv, and submission argv.
It stops before Bazel or BB submission. No source instrumentation is installed
during preparation. Execution installs the prepared runner bytes and restores
the original bytes in `finally`, including failed collection.

The receipt binds both scripts, source, config, selection, instrumentation, and
exact submission argv. Every dry-run invalidates the old receipt before config
parsing. Successful completion replaces it atomically. Reusing an evidence
directory, changing an argument, or failing preparation leaves no usable prior
success. Submission reports the differing receipt fields when it refuses stale
evidence.

To submit an independently authorized measurement, omit `--dry-run` and choose
a new submission output directory. Keep the config, source, receipt, and runner
log arguments identical. Submission checks BB-HOLD immediately before its one
BB submission. Timeouts wait on that invocation; they never submit a replacement.

The exact BB command uses the approved `python3 -I .../bbrun.py` client. Terminal
inspection uses `python3 -I .../bb_logs.py`. Named CAS downloads use the approved
bbrun artifact operation and verify the URI's SHA256 and byte length.

## Evidence and verdicts

The driver stages exact source identities, selections, runner patch and hash,
command, selected-child test.log and XML, and inventory, verifier, and measurement
JSON. Every staged file is a new owned regular file with mode 0600 under a new
0700 directory. It rejects symlinks and special files without changing input
modes. Bazel's top-level testlogs link is the sole deliberate link boundary.
ZIP collection reads only named ordinary receipts and rejects duplicate or
special members. Ordinary mode never copies AIR or a complete test.outputs tree.

An omitted verifier `success` field is allowed only with positive exact count,
selection, zero-error, and error-flag checks. Explicit false fails. Missing
resources remain null and UNKNOWN; numeric zero remains zero. `rlimit_run_sum`
uses only `times-ms.smt.rlimit-run`, with init retained separately.

Terminal collection checks the same own parent, verifies all curated CAS bytes,
rechecks preparation identities and raw child verdicts, and retains attribution
in `terminal-attribution.json`. Command, artifact handoff, scanner, cleanup,
wrapper, parent, driver collection, and source restoration remain separate.
A successful child or client exit cannot establish overall success. Exit 3 means
qualification is UNKNOWN. Invalid terminal evidence fails collection.

The consumer uses ops wrapper-result v1, `tl3.bbrun.wrapper-result`, version 1.
Ops owns `bbrun-wrapper-result.v1.json`, its authoritative schema, sanitized
reason vocabulary, scanner, and cleanup. This directory defines no competing
schema and installs no wrapper. The consumer rejects duplicate keys, unknown
versions or reasons, extra or missing fields, booleans as exit codes,
contradictory stages, and a mismatched parent ID. A nonzero first scan followed
by a genuinely clean final scan is allowed.

A receipt is read only after the own parent is independently terminal. Own CAS
attribution and driver namespace evidence are required. The remote command
checks that the wrapper's reserved name is absent before and after its work.
A single wrapper-owned fallback line from the retained own terminal runner log
is supported; repeated, embedded, or disagreeing markers fail. Missing namespace
evidence prevents log recovery. A missing v1 producer stays UNKNOWN.

`fixture-check.py` uses expressly fake wrapper verdicts and fake CAS responses.
They exercise the consumer, not the scanner. They never upgrade retained RP
results. Retained parent `d765f39c-35dd-4dcd-b013-3005b046c59c` remains FAILED,
and its initial scanner reason remains UNKNOWN. Real scanner regression belongs
to ops. Actual verifier qualification and captain merged-head tooling gates are
outside this producer fixture scope.
