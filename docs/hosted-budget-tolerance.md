# Hosted test budget tolerance

## CPU budgets and the wall backstop

Linux tests use the `cpu` limits in `tools/ci/rust-test-budget.json`.
`execute_binary` samples `utime + stime + cutime + cstime` from the owned
`/proc/<pid>/stat` at each completion marker. Deltas include CPU used by
children the test reaped, such as git and worker Python. The first marker
includes startup. Custom binaries charge remaining work to `__tail__`.
Libtest charges teardown to its last test. The runner samples the final
zombie before reaping it, so process exit cannot erase the tail counters.

`inventory.json` retains wall `seconds` and adds `cpu_seconds` and
`cpu_platform`. CPU limits have no hosted multiplier. A scenario must meet
both its CPU limit and a wall backstop of 10 times its existing effective
wall limit, including the fixed runner multiplier below. For the default
BuildBuddy budget this means 29 CPU seconds and a 165 wall second backstop.
Bazel's target timeout still stops binaries that never finish.

The policy lists every wall-only exception and its reason.
`tl3-store/postgres::*` and `tl3-store/fixture_lifecycle::*`, including their
tails, keep their original effective wall limits because their primary work
runs in daemonized Postgres processes. Darwin and Windows also retain wall
limits because Linux procfs is unavailable. Other platforms and missing or
invalid Linux samples fail closed. Controller tests budget their own logic
and reaped workers. Their incidental Postgres work is outside CPU accounting
and remains bounded by the wall backstop.

Wall limits, CPU limits, their slow lists, and wall-only exception lists may
only shrink against the target branch. Removing CPU enforcement also fails.
CPU seconds can exceed wall seconds when children run in parallel, so the
initial CPU calibration uses its own units. Two new publishing receipts set
the default CPU cap and its observed slow caps to twice the maximum CPU
sample, rounded up to a whole second, with a one second floor. The default
retains at least the previous numerical default. Unmeasured slow caps retain
their previous numerical limits. After initialization, CPU caps only shrink. This is a conservative
initial ceiling, not a measured population percentile.

`docs/bazel-budget-calibration.json` records the receipts, input digests,
method and results. Regenerate its numerical summary from downloaded
`inventory.json` files with:

```sh
python3 tools/ci/bazel_budget_calibration.py --cpu-receipts run1.json run2.json
```

Marker sampling attributes CPU at the time the runner reads each marker.
Buffered markers can move CPU between adjacent scenarios. Counters have
clock-tick resolution, so very short scenarios can record zero CPU seconds.
CPU in living children is charged when they are reaped. Work in persistent
servers cannot be attributed this way. Receipts preserve both measurements
so these limits can be audited.

## Historical wall tolerance

`rust_gate.py` applies fixed multipliers to the existing per-test budgets on
GitHub-hosted Linux. It uses `RUNNER_ENVIRONMENT`, `RUNNER_OS` and
`RUNNER_ARCH`, supplied by Actions, to select the class. Local and fleet runs
keep multiplier 1.0. Unmeasured hosted architectures fail closed. Timing
artifacts retain raw seconds and record the selected class and multiplier.

| Class | Complete runs | Partial runs | Test observations | Largest elapsed / base budget | Multiplier |
| --- | ---: | ---: | ---: | ---: | ---: |
| hosted-x64 | 12 | 0 | 5,277 | 1.125107 | 1.25 |
| hosted-arm64 | 10 | 2 | 5,604 | 1.022898 | 1.15 |
| buildbuddy-amd64 | 10 | 0 | 18,310 | 1.486524 | 1.65 |

The sample is the latest 12 hosted light-check runs with timing artifacts at
collection, spanning 2026-10-31 to 2026-11-01. It includes failed and cancelled
workflow runs. Two ARM runs lack one shard each; their observations remain in
the audit, but their incomplete maxima are excluded from calibration.
`hosted-budget-calibration.json` records run IDs, coverage, overruns and input
digests. Each run is accessible at
`https://github.com/RobTand/trustladderiii/actions/runs/RUN_ID`.

For each complete run and class, take the largest elapsed / base-budget ratio.
Use the empirical nearest-rank p99 of those run maxima, multiply by 1.10,
then round upward to the next 0.05, with a floor of 1.0:

```python
ceil(max(1.0, p99_run_max_ratio * 1.10) * 20) / 20
```

With only 10 to 12 complete runs, this empirical p99 equals the observed
maximum. It does not establish the population p99. The 10% margin allows
some additional hosted variance. Pooling all tests would hide the failures:
the pooled p99 ratios are only 0.558409 on x64 and 0.531852 on ARM.
The helper requires at least ten complete class runs.

These are reviewed, fixed class factors. CI never learns from its current
run or downloads historical data to adjust a threshold. The original wall budget
limits are unchanged. Its target-branch comparison still rejects growth of any
base limit or the explicit slow list before applying tolerance. Reducing a
base limit also reduces its effective hosted limit. Any future class-factor
change needs new measurements and review.

The four observed overruns pass with these factors. The x64 scope scenario
at 11.251s now has a 12.500s effective limit. ARM's synchronous token bridge
at 15.343s now has a 17.250s effective limit. A wall-only default-budget test above
12.500s on x64 or 11.500s on ARM still fails. The Bazel Gate reruns a
budget-only failing target once. Functional failures never receive this rerun.
Functional failures, skipped tests and missing results still fail.

## Reproduce the calibration

List artifacts using REST for every run ID recorded in the JSON:

```sh
gh api repos/RobTand/trustladderiii/actions/runs/RUN_ID/artifacts --paginate
```

Download each `rust-times-INDEX-ARCH` artifact into a fresh directory, preserving
its run ID in the filename:

```sh
gh api repos/RobTand/trustladderiii/actions/artifacts/ARTIFACT_ID/zip > RUN_ID-rust-times-INDEX-ARCH.zip
python3 tools/ci/budget_variance.py /path/to/downloads
```

The helper reads JSON directly from ZIPs without extracting paths, checks shard
metadata and reports SHA-256 digests for the policy and sorted input manifest.
Artifacts currently expire after seven days, so the committed summary retains
the numerical evidence after their expiry. The helper audits this workflow's
alternating ARM-even, x64-odd shard layout.

## BuildBuddy remote execution

The historical wall calibration applies to wall-only scenarios and to the
wall limit used to compute the backstop. Bazel runs each Rust binary through `tools/bazel/rust_runner.py`, which takes
its class from `TL3_RUNNER_CLASS` in `.bazelrc`. The `buildbuddy-amd64` row
above comes from ten uncached runs of every Rust test target in one invocation
(`--runs_per_test=10 --nocache_test_results`), read by
`tools/ci/bazel_budget_calibration.py` with the same formula. Its worst run
maximum was `workers::maker_failure_fix-killed` at 22.298s against 15s. That historical summary is no longer available in the checkout.
`bazel-budget-calibration.json` now records the CPU migration evidence.

arm64 executors are not calibrated: the same procedure produced nine complete
runs, with one `workers` run timing out at 900s and `publishing` scenarios at
up to 4.5 times their budgets. Until ten complete runs exist, the arm64
post-merge run sets `TL3_BUDGETS=record`: it enforces results and keeps each
test's wall and CPU seconds in its receipt, but enforces no budget.
