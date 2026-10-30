# TL3 CI runners

GitHub stopped starting hosted CI at 18:34 UTC on 2026-10-30 because of its
account billing limit. Rob approved this phase with "Just prioritize getting
things running locally, please." That instruction authorizes installing these
runners and moving the existing checks. This is part of #8, not completion of
its Kani and network-isolation requirements.

## Design and isolation

Six TL3 and two sandbox systemd user services on dl380g10 each run a rootless
Podman container. Both repositories use the same image, slot script, and unit
template. The installer selects the repository with its second argument.
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
The existing workflow installs Python 3.14 and uv. Job names, steps, and tests
stay unchanged. Both `runs-on` values change. Rust's overall job budget rises
from 20 to 30 minutes: the cold canary on two CPUs was still passing worker
integration tests when GitHub canceled it at 20 minutes. Test deadlines and
assertions stay intact. Python keeps its 20-minute job budget.

## Capacity

Each container has `--cpus=2 --memory=6g --memory-swap=6g` and at most 1,024
processes. The six TL3 slots use at most 12 CPU equivalents and 36 GiB RAM, enough for
three Mergify batches with two jobs each. Two sandbox slots add at most four CPU
equivalents and 12 GiB RAM. All eight slots total 16 CPU equivalents and 48 GiB. Native thread limits remain in the
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

## Serve the sandbox

Install two separate slots without restarting TL3 jobs:

```sh
deploy/ci-runners/install.sh 2 sandbox
gh api repos/RobTand/tl3-sandbox/actions/runners \
  --jq '.runners[] | {name,status,labels:[.labels[].name]}'
```

The installer reuses the cached `localhost/tl3-ci-runner:phase1` image and
installs the same unit template as `tl3-ci-runner-sandbox@.service`. Each
service passes its prefix to the shared slot script. Its containers are named
`tl3-ci-runner-sandbox-N`; runner names start with `tl3-sandbox-dl380g10-slot-N-`.
Registration uses `POST repos/RobTand/tl3-sandbox/actions/runners/generate-jitconfig`
and labels `self-hosted`, `linux`, `x64`, and `tl3-sandbox-fleet`.
Only sandbox slots 1 and 2 are accepted. Limits and isolation match the TL3 slots.

Each profile has its own environment file under `~/.config/tl3-ci-runners/`,
named after the service prefix. A TL3 reinstall migrates the original
`runner.env`, preserving its CPU placement. A sandbox reinstall preserves
`tl3-ci-runner-sandbox.env` and does not edit TL3's environment or unit.
`install.sh 0 sandbox` updates files without enabling additional slots.
Reinstalling does not restart active services.

Open the sandbox workflow PR as TL3's GitHub App using `sandbox_gate`, as
[the sandbox procedure](../sandbox/RUN_TRIALS.md) describes. Select
`[self-hosted, linux, x64, tl3-sandbox-fleet]` and keep the job name
`Sandbox tests`. Its own PR workflow is the canary. Confirm its successful
jobs API record names a `tl3-sandbox-dl380g10-slot-` runner. Give the PR number
and full head SHA to the coordinator for Rob's code-owner approval and queueing.
Do not submit a human approval or bypass the gate.

Update `deploy/sandbox/files/.github/workflows/tests.yml` and its SHA-256 in
`deploy/sandbox/approved.json` together. After the sandbox PR merges, run the
GET-only settings checker through PrismaBuild. Until then, a live workflow
hash mismatch is expected. The approved baseline remains reviewed, not learned
from live settings.

Run the offline runner boundary tests and sandbox checker tests through
PrismaBuild. `test_runners.py` uses synthetic JIT input and temporary command
boundaries, with no credentials or running containers. Live installation and
the GitHub canary prove the actual registration and image.

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

To roll back the sandbox, revert its workflow through the App and reviewed
queue process, and update the approved copy and hash in the TL3 PR. Stop and
disable only its two slots:

```sh
systemctl --user stop 'tl3-ci-runner-sandbox@*'
systemctl --user disable tl3-ci-runner-sandbox@{1..2}.service
```

Stopping cancels running jobs. The TL3 services keep running. To recover a failed
sandbox slot after fixing its cause, run `systemctl --user reset-failed
tl3-ci-runner-sandbox@N` and then start it.

Reverting restores hosted labels; hosted CI still needs Rob to clear the billing
limit. Images and installed files can remain for a later rollout.

## Phase 2

- Block LAN access while allowing the GitHub, Rust, Python, uv, and package
  endpoints the checks need. Rootless networking alone does not provide this.
- Account for CI's 16 CPUs and 48 GiB in PrismaBuild's capacity and coordinate
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
