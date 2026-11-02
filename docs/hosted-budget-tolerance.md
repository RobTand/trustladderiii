# Hosted test budget tolerance

## CPU budgets and the wall backstop

Linux tests use the `cpu` limits in `tools/ci/rust-test-budget.json`.
`execute_binary` samples `utime + stime + cutime + cstime` from the owned
`/proc/<pid>/stat` at each completion marker. Deltas include CPU used by
children the test reaped, such as git and worker Python. The first marker
includes startup. Custom binaries charge remaining work to `__tail__`.
Libtest charges teardown to its last test. The runner samples the final
zombie before reaping it, so process exit cannot erase the tail counters.

The runner also samples the embedded Postgres postmaster. It looks only in
`tl3-store-pg-*` directories under the test's `TMPDIR`, selects fixtures whose
`fixture.pid` names the test process, and reads `data/postmaster.pid`. Each
postmaster sample sums the same four counters, including backends reaped by
the postmaster. PID and process start ticks identify a server across markers.
The postmaster delta is added to the scenario's CPU total. Receipt field
`postgres_cpu_seconds` shows that contribution separately.

`inventory.json` retains wall `seconds` and adds `cpu_seconds`, `cpu_platform`,
and `cpu_sampling`. Receipts using this accounting have
`cpu_sampling: proc-postmaster-v1`. CPU limits have no hosted multiplier. A
scenario must meet both its CPU limit and a wall backstop of 10 times its
existing effective wall limit, including the fixed runner multiplier below.
Bazel's target timeout still stops binaries that never finish.

The fixture startup diagnostic tells the runner that a server is expected.
An unavailable postmaster PID after startup, an unreadable fixture sample,
or a previously sampled server disappearing makes that scenario wall-only.
Scenarios that never start a server retain CPU enforcement. `cpu_fallbacks` records the reason for every such
scenario, including setup or cleanup markers without a live server. Its
original effective wall limit applies. Calibration excludes these partial
CPU samples. The policy's static `wall_only` scenario list is empty. Darwin
and Windows retain wall limits because Linux procfs is unavailable. Other
platforms and missing or invalid test-process CPU samples fail closed.

CPU overruns and wall-only budget overruns can receive the Gate's one
budget-only rerun. A wall-backstop overrun starts with
`Per-test wall backstop exceeded` and never writes `budget-overrun.txt`, even
if CPU also overran. A possible hang therefore does not consume a second run.

Wall limits, CPU limits, their slow lists, and static wall-only exception
lists may only shrink against the target branch. Removing CPU enforcement
also fails. CPU seconds can exceed wall seconds when children run in parallel.
CPU calibration takes per-scenario maxima from complete receipts, excluding
fallbacks. Measured CPU caps use twice the measured maximum, rounded up to a whole
second with a one second floor. After bootstrap, updates are capped by the
previous limit.
The default uses twice the nearest-rank p95 of measured scenario maxima,
excluding tails, with the same rounding, floor and shrink-only rule.
Unmeasured explicit caps retain their previous numerical limits and are
listed as uncalibrated. An empirical p95 from this small sample does not
establish a population percentile.

The coordinator explicitly authorized one reset of measured caps inside
unmerged PR #466 because adding postmaster CPU changed the measured work.
Main had no CPU policy yet. `--bootstrap-postmaster` records this bootstrap
and requires receipts with full Postgres accounting. It is not a precedent
for growing merged CPU caps. The target-branch growth guard still rejects
all CPU growth once that policy is merged. Unmeasured explicit caps are
retained; fallback samples are never used to reset a cap.

These CPU caps are calibrated on BuildBuddy amd64 only. GitHub-hosted x64
and arm64 need their own CPU receipts before these caps can be trusted for
those executors. The historical wall multipliers below remain on the wall
backstop and on wall-only fallbacks.

`docs/bazel-budget-calibration.json` records the receipts, input digests,
measured coverage, uncalibrated caps and results. Regenerate its numerical
summary from downloaded `inventory.json` files with:

```sh
python3 tools/ci/bazel_budget_calibration.py --cpu-receipts run1.json run2.json
```

Marker sampling attributes CPU when the runner reads each marker. Buffered
markers can move CPU between adjacent scenarios. Counters have clock-tick
resolution, so very short scenarios can record zero CPU seconds. CPU in
living child processes or backends is charged when their parent reaps them.
A postmaster that exits between markers loses its final unsampled CPU; the
receipt records a wall fallback for that scenario. Receipts preserve both
measurements so these limits can be audited.

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
