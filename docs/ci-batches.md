# CI once per merge batch

This document records the earlier fleet rollout. TL3 became public on
2026-10-31 and now runs ordinary PRs and Mergify batches on GitHub-hosted
ARM and x64 runners. See [Hosted CI](hosted-ci.md) for current triggers,
shards and cache ownership. The merge queue and required check names below
remain unchanged; optional PR CI and fleet routing are historical.

The coordinator reviews a full PR head SHA and its PrismaBuild evidence, then
runs `python3 tools/queue_pr.py NUMBER FULL_HEAD_SHA`. The helper uses REST,
checks the head and mergeability, and posts the queue command without reading
PR checks. A changed head needs another review. GitHub can return unknown
mergeability immediately after a push. Retry once GitHub has computed it.

## Why this preserves tested commits

Mergify tests a draft batch, then fast-forwards `main` to that batch's head.
Both jobs check out the head SHA explicitly. They do not test GitHub's
synthetic PR merge ref. `Rust` and `Python` remain GitHub Actions required
checks on `main`, with non-strict matching and their existing app ID.

`branch_protection_injection_mode: merge` keeps those requirements at merge
time instead of adding them to admission. The PR's own head needs no CI.
The queue stays serial, with three speculative batches. External merges
still reset speculation. Failed batches still split to identify the culprit.

The queue uses a fixed target of eight PRs and a four-minute maximum wait.
A dynamic range starting at one would favor singles when slots are free.
Eight PRs can start immediately; smaller batches start when their wait ends.

Classic protection currently has no required-PR/review rule, so fast-forward
does not need a Mergify bypass. No ruleset exists. We enabled administrator
enforcement so an admin cannot push an unchecked commit through the previous
exception. Before/after REST responses are in `ci-batch-evidence/`.

Sources, accessed 2026-10-31:

- [Mergify merge strategies](https://docs.mergify.com/merge-queue/merge-strategies/)
  describes fast-forwarding to the tested batch head and the required-PR bypass.
- [Queue rules](https://docs.mergify.com/merge-queue/rules/) defines protection
  injection modes and the conditions that force draft batch checks.
- [Batches](https://docs.mergify.com/merge-queue/batches/) describes the wait
  window and automatic failure splitting.

## Triggers and cache ownership

Queue drafts always run both jobs. Ordinary PRs allocate no runners by default.
GitHub still creates a workflow record with skipped jobs because its PR event
filters cannot select a head-branch prefix. Skipped jobs have names ending in
`(not requested)`, so they cannot satisfy the required `Rust`/`Python` checks.
A `ci` label opts a PR into checks on subsequent pushes;
adding it starts checks immediately. `workflow_dispatch` runs both jobs at
the selected ref. Remove `ci` after optional debugging.

A push to `main` only runs `Warm Rust cache`. It compiles all test targets
with `--no-run` and builds the deployment helper. It executes no test suite.
All three jobs use the same lockfile-scoped prefix and `shared-key: rust`, so
the warmer publishes the dependency cache that both jobs restore. Existing
main Rust caches already use the `rust` job key, so this preserves their
namespace instead of forcing a cold migration. Compiler or lockfile changes
can still require a cold build. The warmer's 15-minute budget is separate
from the gates.

PR caches belong to their PR merge ref. Other drafts cannot restore them.
The default branch cache remains readable across drafts, so deleting every
main job would eventually make dependency builds cold. Rust-cache retains
dependencies by default, not workspace crates. The warm job cannot eliminate
recompilation of changed TL3 code or speed up integration test execution.

Sources:

- [GitHub cache access restrictions](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching#restrictions-for-accessing-a-cache).
- [Pinned rust-cache inputs and behavior](https://github.com/Swatinem/rust-cache/blob/d1527c8f67ef9af357290475d4af4b0fab8f12e0/README.md).
- [GitHub workflow event filters](https://docs.github.com/en/actions/writing-workflows/workflow-syntax-for-github-actions#onpull_requestpull_request_targetbranchesbranches-ignore).

## Four CPUs per job

Worker pytest uses pinned `pytest-xdist==3.8.0` with `-n 4`. Each process has
its own Python globals, monkeypatches and session fixtures. Files use pytest
temporary directories; the tracing receiver binds a dynamically assigned
loopback port. The tests use fixture models and block external model calls.
The harness suite and standalone unittest suites retain their own execution.
Qualification must run worker tests on dl380g10, sparky and sparklina because
they include subprocesses and socket deadlines. A pass is evidence for the
observed runs, not proof that a suite can never flake.

The four-process qualification passed all 699 tests twice on dl380g10 in
238.85s and 260.36s, and once each on sparky and sparklina in 85.91s and
85.37s. PrismaBuild used four CPUs and a six-GiB memory budget per action.
These runs used physical cores and do not establish CI's SMT performance.
Terminal records and CAS receipt references are in
`ci-batch-evidence/prismabuild.json`.

After merging integration PR #124, the expanded 789-test worker suite passed
on dl380g10 in 247.40s, sparky in 77.24s, and sparklina in 77.71s. The full
Python check passed in 387.12s, including 99 harness tests, all worker tests,
and standalone suites. It skipped the same optional ShellCheck test. That
test and its scripts are unchanged from the earlier scoped ShellCheck pass.

Rust already uses `RUST_TEST_THREADS=4` and `CARGO_BUILD_JOBS=4`. Cargo runs
integration executables in sequence, including custom `harness = false`
executables. Several run `uv sync` against the same `workers/.venv` and then
launch it. Parallelizing those executables would introduce shared writes
while other tests use that environment. Keep their execution until a separate
change gives them isolated environments and qualifies every host. We do not
claim an unmeasured Rust test-phase speedup.

OpenMP, MKL, OpenBLAS, Rayon and Tokio worker limits are one per subprocess.
These limits do not replace the runner's four-CPU container budget.

## Baseline and live rollout

Baseline [batch run 41149223288](https://github.com/RobTand/trustladderiii/actions/runs/41149223288)
ran Rust for 25m55s and Python for 19m02s, totaling 44m57s of runner time.
The workflow took 31m59s including queueing. Worker pytest passed 699 tests
in 1021.50 seconds. Rust clippy took 10.36 seconds and test compilation 46.16
seconds. Most Rust time was execution, not compilation. Exact job timestamps
are saved in `ci-batch-evidence/baseline-jobs.json`.

Three identical runs would use 134m51s of runner time per PR. This is an
extrapolation, not a measured end-to-end PR. With two PRs in a single batch,
the old pattern could use six runs and the new pattern one full run plus a
build-only warmer. Actual new wall time, warmer time and batch runner minutes
must come from the live trial. PrismaBuild runs on physical cores do not
measure CI's SMT placement or GitHub cache transfer costs.

The coordinator must land the config PR through the current queue first.
Its optional checks can be requested with the `ci` label or a dispatch at its
branch. Do not queue it before review. Do not change another agent's queued
PR or draft to force a trial.

After it lands:

1. Create two independent one-line doc PRs from the new `main`, without `ci`.
2. Record that Rust and Python were skipped and used no runner.
3. Review their exact heads and queue both within four minutes.
4. Record the single draft's PR number, both embarked PR numbers, head SHA,
   workflow run ID and job timings. Do not count a split or reset as a success.
5. Read both PRs' REST records and the draft head's check runs. Require both
   PRs merged, both GitHub Actions checks completed successfully, and `main`
   equal to that exact head. Capture promptly before another batch advances it.
6. Read the main warmer's job timings and the next draft's restore/build steps
   to measure cache reuse. Report a failed warmer as a cache failure.

Use only `gh api` REST calls. Relevant endpoints are `pulls/NUMBER`,
`branches/main`, `commits/SHA/check-runs`, `actions/runs?head_sha=SHA`, and
`actions/runs/RUN_ID/jobs?per_page=100`. Save full responses before computing
timings. No completed live trial is claimed by this document.

## Rollback

Revert the config PR through a reviewed PR. Restore the old queue helper
only after its workflow and queue configuration have reverted. Optional CI
can run the revert PR before it enters the queue. Do not reset `main`, force
push batch branches, or cancel another agent's checks. A queue configuration
change can reset speculation, so coordinate that revert with the coordinator.

Administrator enforcement is the only protection change. To restore the
exact prior setting, use:

```sh
gh api -X DELETE repos/RobTand/trustladderiii/branches/main/protection/enforce_admins
gh api repos/RobTand/trustladderiii/branches/main/protection
```

Compare that response with `ci-batch-evidence/protection-before.json`.
The required checks remain Rust and Python throughout. Keep administrator
enforcement unless deliberately restoring the old unchecked-admin exception.

## Coordinator brief changes

Keep PrismaBuild admission, test-first commits, review of the exact SHA and
one push per review round. Replace the PR-CI wait and external helper path
with `python3 tools/queue_pr.py NUMBER FULL_HEAD_SHA`. Do not use the old
direct-merge fallback. Queue reviewed PRs promptly so the four-minute window
can collect them. Ordinary PR CI is optional; batch CI is mandatory.
The old wave-5 statement that GitHub CI cannot start is obsolete. Self-hosted
Mergify CI now runs on the fleet, directly under Rob's amended policy.
