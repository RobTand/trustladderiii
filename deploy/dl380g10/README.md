# Deploy TL3 on dl380g10

This deployment uses Rob's user manager and lingering. It needs no sudo.
Run one installer at a time. Never read credential files into an agent session.
The scripts suppress child output on failure because it can contain credentials.

## Prepare and start

Build and validate from a clean commit through PrismaBuild, tagged `dl380g10`.
`verify.sh` runs config tests, builds the three binaries, tests the installer,
and runs `systemd-analyze --user verify` with temporary executable fixtures.
Reserve 2 CPUs and 6 GiB, with `CARGO_BUILD_JOBS=2` and native threads bounded
to one. Set `TL3_DEPLOY_ARTIFACTS` to a new absolute directory outside the repo
to export `tl3`, `tl3d`, and `tl3-deploy` on dl380g10. No credentials enter PB's snapshot.

Then run on dl380g10, replacing `ARTIFACTS` and `IMAGE` with the admitted build
directory and the installed harness's immutable image ID:

```sh
python3 deploy/dl380g10/deploy.py install --binaries ARTIFACTS --image IMAGE
~/.local/share/tl3/bin/tl3-deploy app-check ~/.config/tl3/tl3.toml
~/.local/share/tl3/bin/tl3-deploy operations ~/.config/tl3/tl3.toml
python3 deploy/dl380g10/deploy.py install --binaries ARTIFACTS --image IMAGE --refresh-config
python3 deploy/dl380g10/deploy.py start
```

`operations` is the only account write in preparation. It uses the existing App
installation, searches all issue pages including closed issues, and matches the
exact title `TL3 operations` and marker `<!-- tl3:operations:v1 -->`.
It never blindly retries an uncertain creation. Rerun to reconcile by marker.
Multiple matches stop the command. The nonsecret `app-state.json` records the
installation and issue numbers for config generation. The operations repo is
not added to the served repositories. Pinning is an optional UI step for Rob.

The installer preserves an existing config unless `--refresh-config` is passed.
Before refreshing a tuned config, preserve its nonsecret settings.
Supply `--primary OPENAI_MODEL --alternate ANTHROPIC_MODEL` to replace the
example's model aliases. Confirm the names, effort values, and maximum output
budgets against CLIProxyAPI before enabling workers. Defaults remain explicit
placeholders until that check is possible with Rob's model credential.
The published PrismaBuild client runs `python3 .../tools/pbrun.py` directly.
The config points to that published tools directory.

Postgres uses the tests' Zonky distribution, pinned to 17.4.0 and a committed
SHA-256, under `~/.local/share/tl3/postgresql-17.4.0`. Data lives under
`~/.local/share/tl3/postgres/data`. TCP is disabled. The owner-only socket
directory uses port 5543 and SCRAM authentication. `initdb` stages a complete
cluster before publication. Existing data and passwords survive reruns.
Startup creates the dedicated `tl3` database and applies TL3's migrations.
`fsync` and synchronous commits stay on. There is no automatic major upgrade.

The installer syncs the locked Python 3.14 venv once per installation. Workers
launch its Python directly. It downloads o200k_base in a new empty directory,
checks SHA-256 `446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d`,
and installs it under the URL's SHA-1 cache key in `~/.local/share/tl3/tiktoken`.
The controller unit exports that directory as `TIKTOKEN_CACHE_DIR`.

## Finish after Rob supplies credentials

The coordinator requests these secrets through T3's private secret prompt and
installs them without displaying their values. Directories have mode 0700.

| Secret | Purpose | Destination on dl380g10 | Mode | Reader |
|---|---|---|---|---|
| Cloudflare tunnel token | Connect the webhook tunnel | `/home/rob/.config/tl3/secrets/cloudflared-token` | 0600 | `tl3-cloudflared.service`, `LoadCredential=token` |
| CLIProxyAPI key | Authorize worker model calls | `/home/rob/.config/tl3/secrets/model-key` | 0600 | `tl3d.service`, `LoadCredential=model-key` |

The App credentials already exist at `/home/rob/.config/tl3/github-app.json`,
mode 0600. Keep them there. The App helper reads them only inside its process.
`tl3d` gets a copy through `LoadCredential=github-app.json`. Its existing
`webhook_secret` signs webhook deliveries. No new webhook token is needed.
The installer generates `/home/rob/.config/tl3/secrets/postgres-password` in
place at 0600 without printing it. Postgres, controller, and status units each
receive it through `LoadCredential=postgres-password`.
Service wrappers write nonsecret runtime configs with their credential-copy
paths. No secret bytes enter a config, command argument, or environment variable.

Rob's steps:

1. In Cloudflare, create a tunnel and a public hostname routing to `http://127.0.0.1:8081`.
2. Give the tunnel token and CLIProxyAPI key through the coordinator's secret prompts.
3. After production startup is available, have the coordinator run the command below with the chosen hostname.
4. In [the App settings](https://github.com/settings/apps/tl3-robtand), check the webhook URL and enable **Active**.
5. Optionally pin the **TL3 operations** issue from its menu.

```sh
~/.local/share/tl3/bin/tl3-deploy webhook-configure ~/.config/tl3/tl3.toml TUNNEL_HOST
systemctl --user enable --now tl3-cloudflared.service
```

`PATCH /app/hook/config` accepts `url`, `secret`, `content_type`, and
`insecure_ssl` using an App JWT. The command sends the existing secret in-process,
checks that the URL persisted, and never displays the request or response.
REST exposes no `active` setting. The initial wizard set it false; the read-only
App check confirms the placeholder URL but cannot independently prove inactivity.
Reference: [GitHub App webhook REST API](https://docs.github.com/en/rest/apps/webhooks?apiVersion=2022-11-28).

## Production startup blocker

At this PR's base, `tl3d` requires `--simulate`, and webhook startup requires
a loopback mock API. Production registration belongs to #20. The installed
controller unit passes `--workers`, and its config supplies loopback webhook
ingress. It never enables simulation against the deployment database.
The unit has a `~/.config/tl3/production-ready` gate. After #20 supplies and
verifies real startup, install that release, validate with `Config::load`,
and create the marker before starting `tl3d`. The daemon and loopback webhook
check remain missing until then. Do not create the marker to bypass this blocker.

`KillMode=process` keeps durable worker supervisors alive during controller
restarts, as #9 requires. Postgres starts before the controller and status.
Each service restarts on failure. Postgres uses a fast, orderly shutdown.
Status binds `192.168.1.107:8033` on `bond0`, never a wildcard. Change the explicit
address if the LAN lease changes. Status uses `Config::load_status` to check
only store credential metadata, so it can run before model setup. Controller
startup still uses full `Config::load`.

## Check the deployment

Run both commands through a PrismaBuild action tagged `dl380g10`:

```sh
systemd-analyze --user --man=no verify ~/.config/systemd/user/tl3*.service
python3 deploy/dl380g10/deploy.py check
```

The check prints service states, schema compatibility, CLI and LAN status,
loopback webhook availability, and missing setup. It returns 1 while anything
is missing. It reads no secret values. `tl3 status` and the store read their
own Postgres credential in-process. No raw logs, config dumps, or HTTP bodies
are printed. After production setup, restart each service and rerun the check
through PrismaBuild to finish #19's acceptance criteria.
