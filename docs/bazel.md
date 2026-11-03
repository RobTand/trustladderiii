# Bazel

Bazel builds every Cargo crate and runs every test the Light checks run, one
target per Rust test binary and per Python test module. Cargo and uv stay the
sources of truth: `Cargo.toml`, `Cargo.lock` and `workers/uv.lock` drive the
generated Bazel inputs.

## Running tests

Run Bazel on BuildBuddy, never with a local Bazel server on a fleet host.
`bb remote` runs the Bazel client on a BuildBuddy runner, mirrors your
uncommitted changes, and sends test actions to remote execution:

```sh
GIT_REPO_DEFAULT_BRANCH=main \
  bb remote \
  --container_image=docker://gcr.io/flame-public/rbe-ubuntu24-04@sha256:5f0c01e5f6d79e58d7a996330de33aa38ce599bf8431eaf27521e7602daadf50 \
  --runner_exec_properties=EstimatedCPU=8 --runner_exec_properties=EstimatedMemory=16GB \
  test //crates/store:test_test_postgres --config=remote --jobs=16
```

Authenticate `bb` through its runtime configuration. Never print a key or put
one on a command line. While iterating, name only the targets you changed,
such as `//workers/tests:test_maker`, and keep `--jobs=16`. The PR Gate runs
the full suite. Read the verdict from the command's exit status and the invocation it links, not from the start message.

Local Bazel is only for short Gazelle, generation and query commands. Put
`startup --max_idle_secs=300` in your own bazelrc (for example one passed with
`bazel --bazelrc=...`), not in the repository's `.bazelrc`.

## Adding a test file

- **Rust:** add `crates/<crate>/tests/<name>.rs` or
  `crates/<crate>/tests/<name>/main.rs`. Bazel finds it by glob; no BUILD edit
  and no generation step. A test that needs a `[[test]]` entry in `Cargo.toml`
  (`harness = false`, or a custom `path`) also needs `tools/bazel/generate.sh`.
- **Python:** add `test_*.py` where the Light checks already collect tests,
  then run `tools/bazel/generate.sh` and commit the BUILD changes Gazelle
  writes.
- **Runtime inputs** that neither Cargo nor Gazelle can see (fixtures in
  another directory, scripts a test starts) go in the tables at the top of
  `tools/bazel/rust.bzl` or `tools/bazel/python.bzl`, keyed by package.
- **A test that cannot be hermetic** goes in `tools/bazel/nonhermetic.bzl`
  with its reason. Remote test actions have no network unless listed there.

CI fails when a generated input is stale (`tools/ci/bazel_generated.py`) or
when Bazel's tests differ from what `cargo metadata` and `tools/ci/check.py`
discover (`tools/ci/bazel_parity.py static`). Both name the missing file and
the fix: `tools/bazel/generate.sh`.

## Configurations

- `--config=remote`: BuildBuddy cache, remote execution on the pinned
  Ubuntu 24.04 image, BES, `--jobs=80`. Development runs override this with `--jobs=16`.
- `--config=remote-arm64`: the same, but every action, including compiles
  and tools, runs on BuildBuddy's linux/arm64 executors. Remote Bazel runners
  are amd64 only, so the client stays on amd64. The post-merge run uses it.
- `--config=ci`: gRPC and compact execution logs, CI build metadata, and the
  per-test receipts parity reads. CI adds `COMMIT_SHA` and `BRANCH_NAME`.

Pass the API key header from the environment or a private rc file. Never commit
it.

## Multi-process test sizes

BuildBuddy Cloud can replace `EstimatedCPU` with a one-CPU prediction. Set
all three properties to retain an explicit reservation:

```python
exec_properties = {
    "EstimatedComputeUnits": "1",
    "EstimatedCPU": "4",
    "EstimatedMemory": "2GB",
}
```

Nonzero compute units disable predicted and measured task sizing. Explicit CPU
and memory override the compute unit defaults of one CPU and 2.5 GB per unit.
`tools/bazel/test_sizing.bzl` applies the sizes in both test macros, after the
network properties from `nonhermetic()`. Rust sizing covers every controller
runner and every package in `_POSTGRES`, including each worker shard. The
harness package default also covers new tests that start nested pytest suites.
`workers/tests:test_pb_tests` also starts a nested harness runner and has its own
measured reservation. Static parity examines every discovered Python module for
subprocess pytest/unittest commands, harness runner paths and harness execution
calls, including scripts written by tests. A newly detected nested suite fails
parity until its measured reservation supplies all three positive properties.
Other Python tests keep dynamic sizing. The sizing helper takes the package as
an argument and loads without Bazel or recorder globals.

The baseline 2026-11-02 execution receipts give these sizing inputs. CPU bursts are the
largest CPU-millisecond delta divided by its wall-millisecond interval in
BuildBuddy's usage timeline, sampled about every 500 ms. They include children,
but can miss shorter bursts. #466's scenario CPU receipts provide a second
check: publishing consumed 473.26 CPU seconds in 397.43 wall seconds. Those
scenario totals are not instantaneous CPU demand.

| Target class | Sampled peak CPU | Peak memory bytes | CPU / memory request |
| --- | ---: | ---: | --- |
| Controller, excluding workers | 2.856 | 980,955,136 | 4 / 2 GiB |
| Workers, per shard | 2.240 | 524,288,000 | 3 / 1 GiB |
| Store/Postgres | 0.912 | 721,035,264 | 2 / 1 GiB |
| Harness/nested pytest | 0.994 | 106,151,936 | 2 / 512 MiB |
| Workers nested pytest, `test_pb_tests` | 1.022 | 433,860,608 | 2 / 1 GiB |

CPU rounds up to a whole core after 25% headroom. Memory gets at least 25%
headroom, rounds up to whole GiB, and has a 512 MiB minimum. The controller
memory maximum comes from GitHub notices, and its CPU maximum from publishing.
Worker measurements cover all eight #483 shards. The unsharded worker target
uses the same reservation; recalibrate if its process concurrency changes.
Store and harness sizes use `test_test_postgres` and `test_report_limits`.
The reservations use representative targets. These samples are amd64 OCI observations. They do not calibrate arm64 budgets.

### Verify the allocation

Use an uncached targeted run, with `--config=remote --jobs=16
--nocache_test_results`. Read the **test invocation ID** printed by Bazel,
inside the outer Remote Bazel runner's log. Then run:

```sh
python3 -I tools/bazel/execution_receipt.py TEST_INVOCATION \
  //crates/controller:test_test_publishing --cpu=4 --memory-bytes=2147483648
```

The tool loads `BUILDBUDDY_API_KEY`, or the private BuildBuddy key file, at
runtime. It calls `rpc/BuildBuddyService/GetExecution` with
`{"executionLookup":{"invocationId":"TEST_INVOCATION"},"inlineExecuteResponse":true}`.
It inspects `executeResponse.result.executionMetadata.auxiliaryMetadata`'s
`ExecutionAuxiliaryMetadata.schedulingMetadata`. `requestedTaskSize` is the
request and `taskSize` is the effective allocation. Compare `estimatedMilliCpu`
and `estimatedMemoryBytes` in both. Platform properties alone are not proof.

Exit zero means every actual test action for that label completed successfully
and both sizes match. The tool excludes XML-generation actions, checks every
shard receipt, and fails if it finds no test execution or a shard is missing.
Pass `--shards=8` for the eight-shard workers target. A cache hit
cannot prove a new reservation; check the target's verdict and shard count in
the run as well. Save its JSON output with the invocation URL as evidence.
For workers use `--cpu=3 --memory-bytes=1073741824`; for Postgres use CPU 2
with the same memory; for harness use CPU 2 and 536870912 bytes.
For `//workers/tests:test_pb_tests`, use CPU 2 and 1073741824 bytes. Its first
512 MiB receipt passed, but the measured 433860608-byte peak left less than
25% headroom. The reservation was raised to 1 GiB. The final
[eb6c1647 receipt](https://app.buildbuddy.io/invocation/eb6c1647-add9-409d-aeb5-9e93779e1484)
shows requested and effective 2000m / 1073741824 bytes; the target and checker
both passed.

The old publishing receipt from [cf925036](https://app.buildbuddy.io/invocation/cf925036-43e2-41c6-be27-3d3276560385)
requested 4000m and 8 GiB but received 1000m and 491,680,192 bytes. Worker
shards in [0a78d9d0](https://app.buildbuddy.io/invocation/0a78d9d0-37f1-41a2-9f4e-17df84f37ba2)
also received 1000m despite their 4000m requests. Before/after receipts are
recorded in `docs/bazel-test-sizing.json`. The worker verification attempt was
repeated alone on reviewed head `8e1d5d97`, without #483's shards, in
[419c494b](https://app.buildbuddy.io/invocation/419c494b-02a7-4cca-876b-3650d56777d3).
The Remote Bazel runner reached its ten-minute deadline before a test verdict.
The remaining action was cancelled and is terminal. Its receipt contains no
allocation metadata, so workers after-sizing proof remains a merge prerequisite.

OCI receipts show the account's shared 30-core quota, not a per-test hard CPU
limit. Sizes control scheduling and CPU weight. Remote configurations retain
80 jobs. Agent development runs explicitly use 16. A measured Gate concurrency
change can follow separately. Do not launch development full-suite comparisons
on the shared executors.

## CI

The Gate workflow (`.github/workflows/gate.yml`) starts
`tools/ci/bazel_gate.py` on a Remote Bazel runner through
`tools/ci/remote_gate.py`, and reads back `gate-result.json`, which the runner
leaves as a BuildBuddy artifact. The gRPC and execution logs come back as a
7-day workflow artifact.

Selection fails closed. The gate fails when the queue-aware base is missing,
bazel-diff fails, a changed file is read by no target and is not in
`IRRELEVANT`, build-relevant changes impact no target, or Mergify does not
receive the scope list. Changes matching `FALLBACK` (Bazel, Cargo and uv
locks and manifests, toolchains, workflows, `tools/bazel/`) test everything
and mark the PR as touching every scope.

Draft merge-queue PRs use `batch-draft` mode. The gate reads complete BEP
JSON lines from a temporary file and sends SIGINT to the Bazel client on the
first non-quarantined FAILED, TIMEOUT, or build failure. Ordinary PRs allow
up to 60 seconds to collect more failures, ending the grace period if Bazel
finishes naturally. `gate-result.json` records `aborted_early` with the first
label and reason, plus `not_run` for selected targets with no test result.
An abort fails the gate and skips further input checks and builds.

Time-budget warnings have PASSED test results and cannot trigger fail-fast.
TL3 currently has no quarantine table. `QUARANTINE` and the workspace's
`quarantine` parameter accept canonical labels when a table is added.

Each push to `main` runs every test on amd64 and arm64. A failure names the
merged PRs and says whether their selection included the failing test. The
job summary and the `post-merge-<sha>` artifact hold one `POST_MERGE_RECORD`
JSON line per run.

## Time budget warnings

Per-test CPU, wall-only, and wall-backstop overruns are warnings. A Rust
binary whose tests pass still passes its Bazel target. Its undeclared
outputs include `budget-warnings.json`, with the test name, measured seconds,
limit, and timing metric. The Python runner has no per-test timing policy.

The gate collects these records as `budget_warnings`, adding each Bazel
label. PRs and queue batches report the warnings in `gate-result.json` and
the job summary. Post-merge results also identify the architecture and retain
warnings in `POST_MERGE_RECORD`.

The gate never reruns a test because of elapsed time. Assertion failures and
Bazel's hard `TIMEOUT` status still fail, with no retry. Recorded budgets and
exception lists still cannot grow against the target branch without review.
