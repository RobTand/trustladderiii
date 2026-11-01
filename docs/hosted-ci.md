# Hosted CI

TL3 is public. Every PR and Mergify batch runs on GitHub-hosted Linux
runners, with read-only repository permissions. No TL3 workflow selects a
fleet runner. Outside contributors still need workflow approval under the
repository's Actions settings. Existing fleet services and registrations are
retained for the coordinator to retire; the private sandbox still uses them.

`HOSTED_TEST_JOBS` in `.github/workflows/light-checks.yml` is the single
default shard count. Ten means eight Rust jobs and two Python jobs. Each
language alternates `ubuntu-24.04-arm` and `ubuntu-24.04`. Rust lint runs
separately. Two compile-only jobs run alongside the test shards: workspace
feature combinations on ARM and Kani proof compilation on x64. Loom supports
only controller library unit tests, so that combination uses `--lib --no-run`;
other combinations build all test targets with `--no-run`. Kani uses
`--only-codegen`, with no proof verification. Each compile job records its
time in an uploaded JSON artifact. The final `Rust` and `Python` checks require every prerequisite
to succeed, including planning and lint. Failed, cancelled, skipped or missing
results fail aggregation. `.mergify.yml` keeps its existing check names.

`tools/ci/hosted-shards.json` stores scheduling weights, separate from test
budgets. Rust weights come from the recorded post-optimization sparky binary
timings in `docs/fast-gate-evidence.json`. Python weights estimate relative
suite cost. Both drivers discover the current inventory before assigning
each suite exactly once with deterministic longest-first balancing. New
suites receive a default weight. The Python shard owning deployment tests
builds their Rust helper; the other shard avoids that build.

Rust shards retain native skipped-test rejection and every per-test budget.
They fetch full Git history and compare policy with the target branch, so
changing the PR's budget file cannot approve a larger slow list or limit.
Existing budget entries are unchanged. Each Rust shard uploads its measured
per-test timings as an artifact. Custom harnesses must print one unique
`test <name> ... ok` line per scenario and exactly one numeric summary,
`N <suite> scenarios passed; ...`. Declare the expected count from the scenario
selection, independently of emitted markers. The gate rejects missing or
duplicate markers and missing or duplicate summaries, naming the binary.
Every runner uses at most four Cargo or
test processes, with native library thread counts bounded to one.

Before timed tests, the fixture helper installs the pinned Postgres archive
once. Its setup duration is reported separately. Every real scenario still
creates its own fresh database and retains its original assertions and time
budget. The archive cache is scoped by architecture and fixture source hash.
This keeps first-use downloads and installation lock contention out of the
first native test's duration, just as worker dependency installation happens
before test timing. The main warmer populates this cache too.

The main-branch cache warmer builds dependencies for both architectures
without running tests. It also caches the pinned Kani compiler and its Rust
toolchain so later proof compilation avoids setup. Rust caches use an architecture-specific shared key
and a lockfile-scoped prefix. PR caches cannot warm unrelated PRs, while
main caches are readable by future PRs. Dependencies are cached; changed
workspace crates still need compilation. A cold build is a separate cost
from the warm-cache gate target of five minutes.

To compare shard counts, dispatch `light-checks.yml` on the same branch with
`test_jobs` set to 8, 10 or 12. Different counts have distinct concurrency
groups so one measurement does not cancel another. Compare completed warm
runs at the same commit, separately recording queue delay, compilation and
test execution. Development verification of these drivers uses PrismaBuild;
the hosted workflow itself runs directly in Actions.
