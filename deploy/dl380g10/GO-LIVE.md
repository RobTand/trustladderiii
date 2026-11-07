# Start the sandbox tracer on dl380g10

This is the coordinator's procedure for #20. The implementation agent must stop
after release installation. Keep the App inactive and the production marker
absent until the coordinator reaches the checks below. Do not print credentials.
Use GitHub REST through `gh api` only. Mergify owns CI and the merge queue.

## Build and install

Use a clean detached checkout of the reviewed commit. All development tests and
builds run through PrismaBuild. Replace `COMMIT` with that full SHA. The artifact
path is unique to this build. The install target requires `dl380g10`.

```bash
git fetch origin
git worktree add --detach /home/rob/tmp/t20-go-live COMMIT
cd /home/rob/tmp/t20-go-live
export TL3_RELEASE=/home/rob/tmp/tl3-release-COMMIT
export TL3_COMMIT=$(git rev-parse HEAD)
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --tag dl380g10 --cpus 2 --demand mem_gb=6 --priority -10 \
  --env PATH=/home/rob/.cargo/bin:/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin \
  --env RUSTUP_HOME=/home/rob/.rustup --env CARGO_HOME=/home/rob/.cargo \
  --env CARGO_BUILD_JOBS=2 --env OMP_NUM_THREADS=1 \
  --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  --detach -- bash deploy/dl380g10/build-release.sh "$TL3_RELEASE" "$TL3_COMMIT"
```

Save the returned action key as `BUILD_KEY`. Check the terminal record, logs,
cleanup and CAS receipt. A submission is not a pass. Verify the three hashes.

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbwait.py BUILD_KEY --endings
(cd "$TL3_RELEASE" && sha256sum -c SHA256SUMS)
```

Choose the operator model settings explicitly before initial installation.
The model-defaults command saves a private config backup and changes the ladders;
it is never part of fresh-init deployment.
Installation preserves the existing model ladders and the webhook port.

```bash
python3 /mnt/shared/prismabuild-fleet/repo/tools/pbrun.py \
  --cwd "$PWD" --tag dl380g10 --cpus 1 --demand mem_gb=2 --priority -10 \
  --env PATH=/home/rob/.local/bin:/usr/local/bin:/usr/bin:/bin \
  --env XDG_RUNTIME_DIR=/run/user/1000 \
  --env OMP_NUM_THREADS=1 --env MKL_NUM_THREADS=1 --env OPENBLAS_NUM_THREADS=1 \
  --detach -- bash -c 'python3 deploy/dl380g10/deploy.py model-defaults && python3 deploy/dl380g10/deploy.py install --binaries "$1" && "$HOME/.local/share/tl3/bin/tl3-deploy" database "$HOME/.config/tl3/tl3.toml" && systemctl --user try-restart tl3-status.service' bash "$TL3_RELEASE"
```

Save this action key and verify it with `pbwait.py --endings`. `deploy.py install`
does not enable or start any unit. It copies the release binaries, Python packages,
unit files and the wrapper. Existing configuration must already contain the current operations identity
and Hindsight inputs; installation never fills or upgrades them.
No credential values enter the checkout or build snapshot.
The database service creates database `tl3` if needed. Fresh-state deployment
initializes the selected revision's complete schema after the verified cold copy
and exact TL3 reset, before starting the controller and its dependents.

## Preflight

Run `model-defaults`, then `check`, on dl380g10. Record the output and exit code.
Before startup, `check` is expected to return 1 for the stopped controller,
webhook listener and missing production marker. Resolve every other missing
check before continuing. The configured listener must be `127.0.0.1:8742`.

```bash
python3 deploy/dl380g10/deploy.py model-defaults
python3 deploy/dl380g10/deploy.py check
systemctl --user is-active tl3-postgres.service tl3-status.service
ss -ltnp 'sport = :8742'
test ! -e "$HOME/.config/tl3/production-ready"
```

TL3 reuses TL2's connector, running as user `tl2`, which routes
`https://tl2-hooks.prismaquant.org` to `http://localhost:8742`. Its availability
is an external dependency. Confirm that connector is healthy before proceeding.

```bash
sudo -n -u tl2 env XDG_RUNTIME_DIR=/run/user/30033 \
  systemctl --user is-active tl2-cloudflared.service
```
Do not install a token for TL3 or start `tl3-cloudflared.service`. The installer
uses the external route by default. `install --managed-tunnel` is only for a
future deployment that owns its own connector. `check` then requires that
connector and its token. Selecting an external route is not proof of public
webhook delivery. The signed ping below provides that evidence.

## Start and verify reception

No seed issue may be labelled yet. The marker must follow startup verification,
so use a temporary runtime-only override for the initial controller start.
This is the only deliberate bypass of the marker. Keep the model-key condition.

```bash
mkdir -p "$XDG_RUNTIME_DIR/systemd/user/tl3d.service.d"
printf '[Unit]\nConditionPathExists=\nConditionPathExists=%%h/.config/tl3/secrets/model-key\n' \
  > "$XDG_RUNTIME_DIR/systemd/user/tl3d.service.d/startup-check.conf"
systemctl --user daemon-reload
systemctl --user start tl3-postgres.service tl3-status.service tl3d.service
systemctl --user is-active tl3-postgres.service tl3-status.service tl3d.service
curl --silent --output /dev/null --write-out '%{http_code}\n' \
  http://127.0.0.1:8742/github/events
journalctl --user -u tl3d.service --since '5 minutes ago' --no-pager
```

The GET must return 405. The log must report production adapters, the webhook
listener, and no source failures. Startup catch-up uses the GitHub App. It may
recover missed events, so verify the sandbox has no previously labelled issue
waiting before starting this procedure. Create the `tl3` label if it's missing,
then confirm that no issue carries it:

```bash
gh api repos/RobTand/tl3-sandbox/labels/tl3 --silent 2>/dev/null \
  || gh api -X POST repos/RobTand/tl3-sandbox/labels -f name=tl3 -f color=1d76db \
       -f description='TL3 request (Rob only)' --silent
gh api 'repos/RobTand/tl3-sandbox/issues?labels=tl3&state=all&per_page=100' --jq 'length'
```

The second command must print `0`.

**Rob step 1.** Enable **Active** in the GitHub App settings. The webhook URL
already points to `https://tl2-hooks.prismaquant.org/github/events`. Keep its
existing secret. Use **Ping** or redeliver a ping in the App's deliveries view.
Confirm the delivery receives HTTP 2xx and the local listener remains healthy.
Record its delivery ID and response status. A curl GET alone does not prove
signature verification or ingress persistence.

## Restart every service, then open the gate

Before any seed is labelled, restart Postgres, status and the controller. The
controller's `KillMode=control-group` stops its writers, Git helpers and PB
clients before a new process starts. Since #372 it no longer stops worker runs:
`workers.launcher` starts each attempt's supervisor in its own scope under
`tl3-workers.slice`, outside tl3d's cgroup. The restarted controller reattaches
to a live supervisor through its claim record, and accepts the run's reply under
the same effect key and generation. A supervisor that is gone goes through the
usual absence proof before any new attempt. To stop every run, use
`deploy.py stop-runs` (below), not a tl3d stop.

```bash
systemctl --user restart tl3-postgres.service
systemctl --user restart tl3-status.service
systemctl --user restart tl3d.service
systemctl --user is-active tl3-postgres.service tl3-status.service tl3d.service
~/.local/share/tl3/bin/tl3 status --config ~/.config/tl3/tl3.toml
curl --fail --silent http://192.168.1.107:8033/ >/dev/null
journalctl --user -u tl3d.service --since '5 minutes ago' --no-pager
```

Verify a second ping or redelivery after the restart. Restart the external TL2
connector through its owning user's service manager, then redeliver once more.
Read-only inspection confirmed this unit and UID. Do not replace its
configuration or token. The service belongs to user `tl2`, so restart it only
with Rob's OK. Rob retired TL2 on 2026-10-30, and its tunnel now serves TL3.

```bash
sudo -n -u tl2 env XDG_RUNTIME_DIR=/run/user/30033 \
  systemctl --user restart tl2-cloudflared.service
sudo -n -u tl2 env XDG_RUNTIME_DIR=/run/user/30033 \
  systemctl --user is-active tl2-cloudflared.service
```

Record the restart result and post-restart delivery ID. If access is unavailable,
record that restart as outstanding and keep the production gate closed.

Workers receive a private attempt directory containing only `model-key`.
The saved request points there rather than into the controller's systemd
credential directory. Its key remains readable across controller restarts until
run retirement deletes the whole attempt. Credentials are never put in a served
worktree, model input text, log or artifact.

After these checks pass, create the marker, remove only the temporary override,
and enable the TL3 units. This does not activate TL3's optional connector.

```bash
install -m 0600 /dev/null "$HOME/.config/tl3/production-ready"
rm "$XDG_RUNTIME_DIR/systemd/user/tl3d.service.d/startup-check.conf"
systemctl --user daemon-reload
systemctl --user enable tl3-postgres.service tl3-status.service tl3d.service
systemctl --user restart tl3d.service
python3 deploy/dl380g10/deploy.py model-defaults
python3 deploy/dl380g10/deploy.py check
```

Now `check` must return 0. Confirm another signed delivery succeeds after the
final restart. The marker remains a permanent condition on the installed unit.

## Run one issue

**Rob step 2.** Label exactly one agreed seed issue `tl3` on
`RobTand/tl3-sandbox`. TL3 must take it to merged and closed without asking Rob
anything. Record its issue number as `SEED` and the resulting PR number as `PR`.

Watch `http://192.168.1.107:8033/` and the controller log. Observe progress,
credit, waits and incidents. Confirm review and checks cover the final head,
the merge happens through Mergify, and the issue gets a plain-language summary.
Also record a successful App branch-ref operation whose branch contains `/`,
as requested by #20's GitHub-library review comment.

```bash
journalctl --user -u tl3d.service -f
~/.local/share/tl3/bin/tl3 status --config ~/.config/tl3/tl3.toml
gh api repos/RobTand/tl3-sandbox/issues/SEED --jq '{state,closed_at}'
gh api repos/RobTand/tl3-sandbox/issues/SEED/comments --jq '.[].body'
gh api repos/RobTand/tl3-sandbox/pulls/PR --jq '{merged,merged_at,head:.head.sha}'
```

Save the issue, PR, final SHA, queue/check evidence, summary and any questions.
Success requires `merged=true`, a closed issue and zero questions to Rob.
The live tracer acceptance and service restart acceptance remain the coordinator's
work; installing this PR does not complete tickets #19 or #20.

## Deployment failure

Stop dispatch and diagnose using the retained diagnostic backup. Select another
qualified build with fresh TL3 state only after reconciling outstanding execution
and cleanup obligations. Follow [FRESH-INIT.md](FRESH-INIT.md). There is no
old-state upgrade or rollback deployment lane.
