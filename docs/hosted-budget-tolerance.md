# Hosted test budget tolerance

`rust_gate.py` applies fixed multipliers to the existing per-test budgets on
GitHub-hosted Linux. It uses `RUNNER_ENVIRONMENT`, `RUNNER_OS` and
`RUNNER_ARCH`, supplied by Actions, to select the class. Local and fleet runs
keep multiplier 1.0. Unmeasured hosted architectures fail closed. Timing
artifacts retain raw seconds and record the selected class and multiplier.

| Class | Complete runs | Partial runs | Test observations | Largest elapsed / base budget | Multiplier |
| --- | ---: | ---: | ---: | ---: | ---: |
| hosted-x64 | 12 | 0 | 5,277 | 1.125107 | 1.25 |
| hosted-arm64 | 10 | 2 | 5,604 | 1.022898 | 1.15 |

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
run or downloads historical data to adjust a threshold. The original budget
file is unchanged. Its target-branch comparison still rejects growth of any
base limit or the explicit slow list before applying tolerance. Reducing a
base limit also reduces its effective hosted limit. Any future class-factor
change needs new measurements and review.

The four observed overruns pass with these factors. The x64 scope scenario
at 11.251s now has a 12.500s effective limit. ARM's synchronous token bridge
at 15.343s now has a 17.250s effective limit. A default-budget test above
12.500s on x64 or 11.500s on ARM still fails immediately. There are no retries.
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
