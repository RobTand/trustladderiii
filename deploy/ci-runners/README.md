# TL3 CI runners

GitHub stopped starting hosted CI at 18:34 UTC on 2026-10-30 because of its
account billing limit. Rob approved this phase with "Just prioritize getting
things running locally, please." That instruction authorizes installing these
runners and moving the existing checks. This is part of #8, not completion of
its Kani and network-isolation requirements.

## Design and isolation

Six systemd user services on dl380g10 each run a rootless Podman container.
The host script calls the repository's `generate-jitconfig` REST endpoint using
the host's existing `gh` login. It sends the resulting one-job credential through
stdin, without writing it to disk, the Podman environment, or the journal.
The host login never enters the container. Do not run these scripts with shell
tracing or enable GitHub CLI debug logging.

Runner names start with `tl3-dl380g10-slot-N-` and have a unique suffix for each
registration. Labels are `self-hosted`, `linux`, `x64`, and `tl3-fleet`.
GitHub's JIT configuration makes the runner ephemeral. The listener exits after
one job, Podman deletes the container, and systemd restarts the slot with a new
registration. There is no status-polling loop. Registration failures back off
for 30 seconds, with at most five starts in ten minutes. If that limit trips,
fix the cause, run `systemctl --user reset-failed tl3-ci-runner@N`, and start it.

Containers have private process, user, and network namespaces. They mount no
host directories, shared storage, credentials, or container sockets. Container
capabilities are dropped, privilege escalation is disabled, and host proxy
environment variables are not inherited. Their writable files disappear with
the container. The host's home and credentials are outside the job filesystem.
The image starts as the unprivileged `runner` user.

Rootless networking still reaches the LAN, including services that might expose
host data or credentials. Phase 1 does not protect against that route or a
container-runtime/kernel vulnerability. Use these runners only for trusted
TL3 PRs. Durable forwarding of runner diagnostic logs also remains future work;
GitHub keeps job logs, while local diagnostics disappear with the container.

The Containerfile pins the official Actions runner base by manifest digest.
It adds build tools, a checksum-pinned rustup installer, Rust 1.99.1 with clippy
and rustfmt, ShellCheck, `systemd-analyze`, and shared libraries for embedded Postgres.
The existing workflow installs Python 3.14 and uv. Job names, steps, tests, and
timeouts stay unchanged; only both `runs-on` values change.

## Capacity

Each container has `--cpus=2 --memory=6g --memory-swap=6g` and at most 1,024
processes. Six slots use at most 12 CPU equivalents and 36 GiB RAM, enough for
three Mergify batches with two jobs each. Native thread limits remain in the
workflow and image. Image builds also have a two-CPU, six-GiB limit.

On 2026-10-30, the published PrismaBuild configuration at
`/mnt/shared/prismabuild-fleet/repo/tools/fleet/fleet_boxes.json` declares 16
worker loops on dl380g10, `--mem-gb 96`, and `--all-cores`. The supervisor has
no systemd `CPUAffinity` or `AllowedCPUs` restriction, and its inherited CPU set
is 0 through 79. There is no unused CPU range on which to place CI. The runner
quota therefore overlaps PrismaBuild's eligible CPUs. This rollout does not
change PrismaBuild's configuration or claim a reservation in its ledger.

If a later fleet allocation provides a separate CPU range, set
`CI_RUNNER_CPUSET=...` when installing. The slot translates that value to
`--cpuset-cpus`. An ordinary reinstall preserves the existing CPU placement.
Restart idle slots to apply image or CPU-placement changes.

## Install and roll out

The owner needs Podman with rootless cgroups v2, GitHub CLI authenticated with
permission to manage repository runners, a working systemd user manager, and
lingering enabled. Run as that owner, without sudo:

```sh
deploy/ci-runners/install.sh 1
```

The installer rebuilds through Podman's cache, installs the scripts and unit,
records the immutable local image ID, reloads systemd, and enables one slot.
Rerunning it converges on the same files without restarting an active job.
Use `install.sh 0` to install without starting slots. This does not stop slots
that are already active.

Open the workflow PR. Its own workflow targets the new runner, so `Rust` and
`Python` run sequentially on the canary slot. Read their jobs API records and
check conclusions, `runner_name`, labels, and check-run links. If GitHub refuses
self-hosted jobs because of billing, stop and report its exact message to Rob.
After both jobs pass, enable the remaining slots without another push:

```sh
systemctl --user enable --now tl3-ci-runner@{2..6}.service
systemctl --user status 'tl3-ci-runner@*' --no-pager
podman image inspect localhost/tl3-ci-runner:phase1 --format '{{.Digest}} {{.Id}}'
gh api repos/RobTand/trustladderiii/actions/runners \
  --jq '.runners[] | {name,status,labels:[.labels[].name]}'
```

The coordinator queues the PR through Mergify. Keep the required check names
`Rust` and `Python`. Do not merge directly or change the queue configuration.

Provisioning has no test-first cycle. The canary's real GitHub jobs verify the
image, registration, workflow, and existing test suites together. CI runs on the
fleet directly under Rob's Mergify policy. Agent-run development tests still
require PrismaBuild; this infrastructure is not an alternative test route.

## Roll back

Revert the workflow commit through the normal reviewed PR process, then stop
the services:

```sh
systemctl --user stop 'tl3-ci-runner@*'
```

That stop command cancels any running jobs. To keep slots stopped after a user
manager restart, also disable the six instances:

```sh
systemctl --user disable tl3-ci-runner@{1..6}.service
```

Reverting restores hosted labels; hosted CI still needs Rob to clear the billing
limit. Images and installed files can remain for a later rollout.

## Phase 2

- Block LAN access while allowing the GitHub, Rust, Python, uv, and package
  endpoints the checks need. Rootless networking alone does not provide this.
- Account for CI's 12 CPUs and 36 GiB in PrismaBuild's capacity and coordinate
  CPU affinity, rather than competing for its full inherited CPU set.
- Add ARM runners on sparky and sparklina after that accounting is in place.
  Pin an ARM runner base and rustup installer with their own digests, validate
  architecture-specific dependencies and all checks, give runners an `arm64`
  label, and choose workflow labels that deliberately permit ARM. Do not put
  the fleet shared mount or sockets inside CI containers.
- Add ticket #8's Kani check with a separate heavier slot type and label. It
  must be required for kernel/proof changes, run fully at night, cap proof
  parallelism at eight, and demonstrably block a failing proof. The six light
  slots in this phase do not satisfy that requirement.
