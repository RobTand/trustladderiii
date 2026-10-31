# CircleCI shadow gate

CircleCI supplements the existing GitHub Actions gate. Mergify still requires
`Rust` and `Python`. Switching required checks needs a separate reviewed PR after
three distinct queue batches agree. No application tests or assertions change.

## Runners

The `robtand` namespace belongs to CircleCI organization
`2c15cbc3-a19b-460c-a28d-8cfacd1d7588`. Only TL3 and PB were configured.

| Resource class | Hosts | CircleCI slots | Use |
| --- | --- | ---: | --- |
| `robtand/fast-x64` | celestia | 3 | TL3 Rust |
| `robtand/fast-arm64` | sparky, sparklina | 1 each | PB Python, ARM qualification |
| `robtand/slow-x64` | dl380g10 | 2 | TL3 Python, PB light jobs |

Celestia GitHub slots 4-6 were idle before being disabled. Slots 1-3 remain on.
Its three CircleCI services keep total CI concurrency at six. At cutover, drain
TL3 GitHub jobs, disable the three remaining TL3 GitHub units on celestia, and
raise CircleCI's celestia capacity only if the account permits it. Keep the
`deploy/ci-runners` files and sandbox services. Rollback disables CircleCI's
celestia slots before restoring all six TL3 GitHub units.

Every CircleCI slot has a four-CPU quota, 6 GiB RAM, 1,024-process limit, bounded
native threads, and a private named cache volume. dl380g10 uses CPUs 40-79.
These are CI limits, not PrismaBuild ledger reservations. No PrismaBuild
runtime, supervisor, live tl3d service, database, or live worktree changes.

Machine runner 3.1.14 runs inside rootless Podman. The listener runs as container
root. Its task prefix drops to the unprivileged `runner` user, with privilege
escalation disabled. Its resource token is a read-only, root-only config file,
stored mode 600 on the host. Jobs mount no owner home, host socket, or fleet
storage. Each runner exits after one task and systemd starts a fresh container.
The listener starts at `/`, outside its disposable `/work/jobs` directory.
Starting it inside the job directory caused `getwd: no such file or directory`
on the first pipeline because CircleCI removes that directory during cleanup.
Only the slot's cache survives. GitHub fork builds and fork secret access are
disabled for both projects.

`deploy/circleci-runners/install.sh` installs files and a separate image without
starting services. It requires the existing native TL3 runner base image,
rootless Podman, an authenticated `gh`, and a systemd user manager. It verifies
checksums for the downloaded runner and nextest binaries. Use
`write_config.py SLOT` with the token-creation API response on stdin to store a
host's config without displaying the token. Start `circleci-fleet@SLOT.service`
only after confirming that the host's capacity permits it. Never include the
personal CircleCI token or a resource token in the repository or job environment.

## Coverage and timing

`CircleCI Rust` has three parallel nodes on `fast-x64`. Each builds the tests;
node zero also checks formatting and clippy. `cargo-nextest` runs all targets
and splits by binary ID. Its JUnit `classname` is that same binary ID, so
`circleci tests split --split-by=timings --timings-type=classname` learns the
sum of each binary's test durations. Each node uses four test threads.

Ordinary Rust branches select changed workspace crates and their reverse
dependencies with nextest `rdeps()`. Shared files, deleted crates, missing Git
history, and unknown inputs select everything. Queue branches, main, explicit
`full=true` pipelines, and nightly runs always select the full suite. The ARM
class receives no TL3 Rust job until #175 qualifies it. There is no measured
speed ratio for the new runner platform yet, so there is no invented weighting
between architectures.

`CircleCI Python` has two parallel nodes on `slow-x64`. It builds the same
Rust deployment helper and checks the same Python projects as Light checks.
Node zero runs Ruff and lock checks. Tests run in the owning project's uv
environment, with standalone unittest files executed by pytest. JUnit file
paths are normalized to repository-relative paths. CircleCI splits those files
using prior file timings. Both jobs upload results with `store_test_results`.
No Python tests are removed by impact analysis in this first shadow gate.

Main pushes run the full gate. The nightly workflow also runs it at 03:15 UTC.
The first timing split has no history and CircleCI falls back to its initial
partition. Warm runs provide the evidence for later balancing decisions.

## Smarter Testing and cost

[Smarter Testing became GA on October 13, 2026](https://circleci.com/changelog/smarter-testing-is-now-generally-available/).
[Test impact analysis](https://circleci.com/docs/guides/test/set-up-test-impact-analysis/)
uses coverage data and supports pytest, Jest, Go, Vitest, RSpec, Mocha, and other
coverage-producing frameworks. Dynamic parallelism is still experimental.
The new command is `circleci testsuite run` with `.circleci/test-suites.yml`.

[Static timing splits and JUnit uploads are free](https://circleci.com/docs/guides/test/smarter-testing-overview/).
Impact analysis and dynamic splitting count discovered test atoms. Monthly
allowances are 1,000 on Free, 10,000 on Performance, and 100,000 on Scale.
Overage costs 3,000 credits per 100,000 atoms, rounded up. These charged features
remain off at the coordinator's direction. Do not start
a trial or upgrade a plan to enable them.

The coordinator confirmed the Free plan's five-task limit on 2026-10-31.
It is shared by TL3 and PB across machine and container runners. The token's
plan endpoints returned 404; [public pricing](https://circleci.com/pricing/)
describes the same five-task allowance. Installed
listeners do not establish the permitted concurrency. Network egress can also
consume credits beyond the account allowance. The platform uses local cache
volumes and uploads only test results, avoiding remote build-cache transfers.

## Compare three batches before switching

Both projects currently have an OAuth `all-pushes` trigger. The API project
settings have `build_prs_only=false`, `build_fork_prs=false`,
`forks_receive_secret_env_vars=false`, and `set_github_status=true`. Queue branch
pushes and draft updates therefore trigger pipelines. No non-draft-only event
filter is installed.

Run this read-only collector after shadow CI reaches main:

```sh
python3 tools/ci/compare_shadow.py --output /tmp/tl3-circleci-shadow.json
```

It joins CircleCI pipelines with Light checks on the exact queue head SHA,
compares Rust and Python separately, and records URLs, conclusions, and wall
seconds. During the next three batches, also record each CircleCI job's
`executor.resource_class`, `queued_at`, and `started_at` from the v2 job API.
Report queue-to-start seconds separately from execution time. Correlate waits
with running tasks across both TL3 and PB before attributing them to the
five-task limit. A queue delay alone does not prove that limit was the cause. Canceled, missing, and unfinished checks are not agreement. A rerun of
one SHA does not count as another batch. Review at least three distinct batches,
including any disagreements, before preparing the cutover.

The cutover PR changes Mergify conditions to the exact CircleCI status contexts
observed on those SHAs and removes the TL3 GitHub Rust and Python jobs. The
coordinator reviews and queues that PR. Do not infer status context names from
job display names. Do not merge it yourself.

PB's lead owns PB's test selection, tiers, full-suite policy, real CircleCI
configuration, and Mergify rules. The platform handoff and minimal example are
posted in [PB #2428](https://github.com/RobTand/prismabuild/issues/2428#issuecomment-6547609574).
