# TL3 CI runners

The current queue tests each batch once and fast-forwards `main` to the tested
head. Ordinary PR checks are optional. Main pushes only build the Rust cache.
See [CI batches](../../docs/ci-batches.md) for the trigger policy, protection
evidence, timings, rollout trial, coordinator instructions and rollback.

GitHub stopped starting hosted CI at 18:34 UTC on 2026-10-30 because of its
account billing limit. Rob approved this phase with "Just prioritize getting
things running locally, please." That instruction authorizes installing these
runners and moving the existing checks. This is part of #8, not completion of
its Kani and network-isolation requirements.

## Design and isolation

TL3 systemd user services run rootless Podman containers on dl380g10,
celestia, sparky, and sparklina. Only dl380g10 also serves the sandbox. Both repositories use the same image and slot script. The
installer selects the repository with its second argument.
The host script calls the repository's `generate-jitconfig` REST endpoint using
the host's existing `gh` login. It sends the resulting one-job credential through
stdin, without writing it to disk, the Podman environment, or the journal.
The entrypoint exports `ACTIONS_RUNNER_INPUT_JITCONFIG` inside the container and
executes `Runner.Listener run`, keeping the credential out of argv. Runner
2.338.2's [`CommandSettings`](https://github.com/actions/runner/blob/v2.338.2/src/Runner.Listener/CommandSettings.cs#L110-L138)
reads `ACTIONS_RUNNER_INPUT_*`, saves the arguments internally, and deletes the
environment variables before starting job processes. Both CI jobs assert that
JITCONFIG is unset and print only `set` or `unset`, never its value.
The host login never enters the container. Do not run these scripts with shell
tracing or enable GitHub CLI debug logging.

Runner names start with `tl3-HOST-slot-N-` and have a unique suffix for each
registration. Labels are `self-hosted`, `linux`, `tl3-fleet`, and
`tl3-host-HOST`, plus `x64` on x86_64 or `ARM64` on aarch64. The optional
`runner_host` workflow-dispatch input targets a host for rollout checks. Its
concurrency group includes that host so concurrent canaries cannot cancel each
other. PR and Mergify jobs keep selecting `[self-hosted, tl3-fleet]`.
GitHub's JIT configuration makes the runner ephemeral. The listener exits after
one job, Podman deletes the container, and systemd restarts the slot with a new
registration. There is no status-polling loop. Start limiting is disabled, so
normal job exits and long registration outages cannot permanently fail a slot.
Registration failures back off from 30 seconds to five minutes over five steps.
After a valid registration, the host sends `RESTART_RESET=1` to systemd, resetting
the next delay to 30 seconds. This also holds after a long successful job.
Systemd 259 does not reset that counter merely because a service runs for a
long time. Its [`service.c`](https://github.com/systemd/systemd/blob/v259/src/core/service.c#L5087-L5095)
handles the explicit notification. `NotifyAccess=all` admits the slot's
`systemd-notify` child; the container receives no host notification socket.

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

The Containerfile pins separate amd64 and arm64 manifests for official Actions
runner 2.338.2. The installer passes `TARGETARCH` from the native architecture.
Each architecture has its own pinned rustup installer checksum. Rustup selects
native Rust components; apt selects native system packages. The pinned
setup-python and setup-uv actions select their own architecture-specific assets.
It adds build tools, a checksum-pinned rustup installer, Rust 1.99.1 with clippy
and rustfmt, ShellCheck, `systemd-analyze`, and shared libraries for embedded Postgres.
The existing workflow installs Python 3.14 and uv. The required check names stay
`Rust` and `Python`. Rust's overall job budget is 40 minutes and
Python's is 30 minutes. Worker pytest uses four isolated processes. Rust keeps
four libtest threads; integration executables retain Cargo's ordering. Native
libraries, Rayon and Tokio workers are bounded. Test deadlines and assertions
stay intact. Mergify allows
60 minutes for checks, including time spent waiting for a free runner.

## Capacity

Each container has `--cpus=4 --memory=6g --memory-swap=6g`
and at most 1,024 processes. Six TL3 slots use at most 24 CPU equivalents and
36 GiB RAM, enough for three Mergify batches with two jobs each. The build-only
cache warmer shares these TL3 slots and can briefly leave a batch job queued.
Two sandbox slots add at most eight CPU equivalents and 12 GiB RAM. All eight
slots total 32 CPU equivalents and 48 GiB RAM. Native thread limits remain in
the workflow and image. Image builds also have a two-CPU, six-GiB limit.

On 2026-10-30, the published PrismaBuild configuration at
`/mnt/shared/prismabuild-fleet/repo/tools/fleet/fleet_boxes.json` declares 16
worker loops on dl380g10, `--mem-gb 96`, and `--all-cores`. The supervisor has
no systemd `CPUAffinity` or `AllowedCPUs` restriction, and its inherited CPU set
is 0 through 79. CI uses dl380g10's SMT siblings, CPUs 40 through 79, leaving
physical cores 0 through 39 preferred for PrismaBuild. PrismaBuild can still
use the siblings as overflow. This rollout does not change its configuration
or claim a reservation in its ledger.

dl380g10 additionally uses `--cpuset-cpus=40-79`. Other hosts default to no
explicit CPU set, retaining their available CPU placement. Set
`CI_RUNNER_CPUSET=...` when installing to override the host default. An ordinary
reinstall preserves existing CPU placement. An absent or empty placement uses
the host default. For the upgrade from #98, run
`CI_RUNNER_CPUSET=40-79 deploy/ci-runners/install.sh 0`.
Restart idle slots one at a time to apply image or CPU-placement changes. Check
the REST runners list for `busy=false` immediately before each restart. If all
slots are busy, wait for a runner's exit through systemd rather than polling or
stopping a job.

## Additional fleet hosts

The 2026-10-31 rollout adds six TL3 slots on celestia and four on each Spark.
Existing dl380g10 slots stay unchanged. Six celestia slots can consume at most
24 CPU equivalents and 36 GiB RAM on its 16 logical CPUs and 61 GiB RAM. Four
Spark slots cap at 16 CPU equivalents and 24 GiB RAM per host. CPU quotas may
share available CPUs; they are ceilings rather than reservations.

The units use `CPUWeight=20` and `Nice=10` for low-priority CI. Celestia gets a
unit drop-in with milder `CPUWeight=50` and `Nice=5`, following Rob's instruction
to use all six slots. Listener processes inherit those nice values. Rootless
Podman may place container scopes beside the service, so nice is also needed
to keep job processes below ordinary feeder processes. None of this changes
PrismaBuild admission, affinities, supervisors, or workers.

On the Sparks, passwordless sudo installed distro-signed Podman, uidmap,
slirp4netns, and fuse-overlayfs packages. No Docker daemon, socket, group,
worker, or GPU configuration was changed. All hosts have an existing owner
login with runner-management access. The Spark CLI lives in `~/.local/bin`;
the scripts add that path. Credentials never move between hosts.

For a fresh Spark, check `sudo -n true` first. If permitted, install rootless
Podman and its helpers, then run `gh auth status` as the owner. Stop that
host's rollout if authentication or runtime setup needs Rob. The installer
requires rootless Podman and refuses a rootful runtime. A Docker adapter was
unnecessary because all three hosts support rootless Podman.

Install as the authenticated owner:

```sh
# On celestia
deploy/ci-runners/install.sh 6
# On each Spark
deploy/ci-runners/install.sh 4
```

Run the same workflow once per host with a branch containing the host input:

```sh
gh api --method POST repos/RobTand/trustladderiii/actions/workflows/light-checks.yml/dispatches \
  -f ref=YOUR_BRANCH -f 'inputs[runner_host]=celestia'
```

Repeat with `sparky` and `sparklina`. Inspect the jobs API for successful
`Rust` and `Python` conclusions and matching `runner_name` prefixes. These are
full batch-equivalent jobs, including the JIT input scrub assertion. Runtime
state and timings are recorded under [evidence](evidence/). No sandbox slots
are installed on ARM hosts; both installer and slot script reject that profile.

Systemd 255 on the Sparks supports restart steps, but does not implement
[`RESTART_RESET=1`](https://www.freedesktop.org/software/systemd/man/latest/sd_notify.html#RESTART_RESET=1),
introduced in systemd 258. Its restart delay can rise to five minutes after
repeated restarts. Slots still recover without polling or exhausting a start
limit. Celestia uses systemd 259.

## Install and roll out

The owner needs Podman with rootless cgroups v2, GitHub CLI authenticated with
permission to manage repository runners, a working systemd user manager, and
lingering enabled. Run as that owner, without sudo:

```sh
deploy/ci-runners/install.sh 1
```

If you set CPU placement, the user manager must delegate `cpuset` along with
`cpu`, `memory`, and `pids`.
On dl380g10, `/etc/systemd/system/user@1000.service.d/50-tl3-ci-cpuset.conf`
contains `[Service]` and `Delegate=cpu cpuset memory pids`. Root's
`systemctl daemon-reload` applies that delegation without restarting the user
manager or its jobs. Reload the user manager afterward. Confirm that
`systemctl show user@1000.service -p DelegateControllers` includes `cpuset`
before setting container CPU placement.

The installer rebuilds through Podman's cache, installs the scripts and unit,
records the immutable local image ID, reloads systemd, and enables one slot.
Rerunning it converges on the same files without restarting an active job.
After a successful retag it updates the image ID in every installed profile's
environment file, preserving each profile's CPU placement. It then removes only
previous image IDs that no environment file references, without force. If a
container still references an old image, removal is deferred. Remove that same
ID after the idle-slot rollout. Never use `podman image prune`, since other
projects have untagged images in the same store.
Use `install.sh 0` to install without starting slots. This does not stop slots
that are already active.

Open the workflow PR and request CI with a workflow dispatch at its branch.
Its workflow targets the new runner, so `Rust` and `Python` run sequentially
on the canary slot. Read their jobs API records and
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

The coordinator reviews and queues PRs through `tools/queue_pr.py` using the
full reviewed head SHA and PrismaBuild evidence. Keep the required check names
`Rust` and `Python`. Do not merge directly. Queue drafts run both checks;
ordinary PRs can request them with a `ci` label or workflow dispatch.

Infrastructure hardening follows test-first through demonstrations. Run
`python3 deploy/ci-runners/test_hardening.py` through PrismaBuild before and
after changes. The dry run substitutes `gh`, Podman, and notification commands
without credentials, executes ten quick exits, and checks disabled start limits
and reset notifications. It also covers the credential handoff, CPU limits,
registration error status, and safe image replacement. A real CI canary verifies
that job steps receive no JITCONFIG, and warm CI jobs measure the CPU placement.
The PR records the red and green action keys and live evidence. CI runs on the
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
installs `tl3-ci-runner-sandbox@.service`. Each sandbox service passes its
prefix to the shared slot script. Its containers are named
`tl3-ci-runner-sandbox-N`; runner names start with `tl3-sandbox-dl380g10-slot-N-`.
Registration uses `POST repos/RobTand/tl3-sandbox/actions/runners/generate-jitconfig`
and labels `self-hosted`, `linux`, `x64`, and `tl3-sandbox-fleet`.
Only sandbox slots 1 and 2 are accepted. Limits and isolation match the TL3 slots.

The unit files reproduce the installed units on dl380g10. TL3 keeps
`~/.config/tl3-ci-runners/runner.env` and the slot script's default prefix.
The sandbox uses `~/.config/tl3-ci-runners/tl3-ci-runner-sandbox.env` and
passes `%i %p`. Both units disable start limiting and use the same restart
backoff and notifications. A sandbox reinstall preserves its CPU placement
and updates only the image ID in TL3's environment, leaving its unit untouched.
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

First stop and disable the units. A revert PR cannot get hosted checks while
GitHub's billing block remains in place. During a planned rollback, let active
jobs exit and confirm `busy=false` before stopping each slot. In an emergency,
the stop cancels running jobs:

```sh
systemctl --user stop 'tl3-ci-runner@*'
systemctl --user disable tl3-ci-runner@{1..6}.service
```

To roll back the sandbox, revert its workflow through the App and reviewed
queue process, and update the approved copy and hash in the TL3 PR. Stop and
disable only its two slots:

```sh
systemctl --user stop 'tl3-ci-runner-sandbox@*'
systemctl --user disable tl3-ci-runner-sandbox@{1..2}.service
```

Stopping sandbox slots cancels their running jobs. The TL3 slots keep running.

Then prepare the revert through the normal reviewed PR process. Rob must clear
the billing limit before its hosted checks can run. Reverting restores hosted
labels. Images and installed files can remain for a later rollout.

## Phase 2

- Block LAN access while allowing the GitHub, Rust, Python, uv, and package
  endpoints the checks need. Rootless networking alone does not provide this.
- Account for CI's 32 CPUs and 48 GiB in PrismaBuild's capacity and coordinate
  CPU affinity, rather than competing for its full inherited CPU set.
- Account for the additional fleet slots in admission. Rob authorized sharing
  all fleet hosts on 2026-10-31. The slots use low priority and bounded resources,
  but do not create PrismaBuild ledger reservations.
- Add ticket #8's Kani check with a separate heavier slot type and label. It
  must be required for kernel/proof changes, run fully at night, cap proof
  parallelism at eight, and demonstrably block a failing proof. The six light
  slots in this phase do not satisfy that requirement.
