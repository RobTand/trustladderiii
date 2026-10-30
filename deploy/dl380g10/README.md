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
python3 deploy/dl380g10/deploy.py install --binaries ARTIFACTS --image IMAGE
python3 deploy/dl380g10/deploy.py start
```

`--image`, `--primary` and `--alternate` only shape a new `tl3.toml`. Once the
file exists, a rerun keeps it and says which flags had no effect. Edit the file,
or run `deploy.py model-defaults`, to change those settings later.

For the same sequence with an issue-identity assertion and service restart
checks, submit `bash deploy/dl380g10/prepare.sh --binaries ARTIFACTS --image IMAGE`
through PrismaBuild with `--tag dl380g10`, 2 CPUs, 3 GiB, and
`XDG_RUNTIME_DIR=/run/user/1000`. It installs twice to prove reruns preserve
credentials and resources. The final check lists the steps still missing.

`operations` is the only account write in preparation. It uses the existing App
installation, searches all issue pages including closed issues, and matches the
exact title `TL3 operations` and marker `<!-- tl3:operations:v1 -->`.
It journals a pending creation before POST and records the returned number
durably. Reruns read that issue directly, so a stale list cannot create another.
A rejected POST with a 4xx response clears `creation_pending`. Fix permissions
or the request and rerun `operations`. Transport errors, invalid successful
responses and 5xx responses keep the pending marker because creation is uncertain.
Rerun discovery first. If the marker remains pending, check GitHub's issue record
including closed issues and confirm that no creation occurred before removing
`creation_pending` from the nonsecret `app-state.json`. Preserve a private backup
first, then rerun. Never clear it based only on elapsed time.
If duplicate App-authored copies exist, it keeps the oldest and closes the others
with a duplicate marker. Copies by another author stop automatic reconciliation.
The nonsecret `app-state.json` records the installation and issue numbers for
config generation. The operations repo is
not added to the served repositories. Pinning is an optional UI step for Rob.

The installer merges missing deployment fields into an existing config. It keeps
operator model names, efforts, budgets, the harness image, and all existing
values. Every rewrite first saves `tl3.toml.TIMESTAMP.bak` at mode 0600. A rerun
with no missing fields leaves the config untouched.

`models.toml` in this directory holds the Q22 model choices as data. New installs
use it. To apply those defaults deliberately to an existing deployment, run
`python3 deploy/dl380g10/deploy.py model-defaults` through PrismaBuild. This
explicit command replaces role ladders, preserves the endpoint and key path,
and saves a 0600 backup. Normal preparation never resets tuned ladders.
The deployed `~/.config/tl3/tl3.toml` remains the operator's runtime config.

The maker ladder uses GPT-6.1 Sol twice, then Claude Fable 5.1. Attempt 2 uses
`max` effort and 128,000 output tokens. GPT makers get Claude Opus 5.5 review;
Claude makers get GPT-6.1 Sol review. The planner and rebaser use GPT-6.1 Sol.
Ingest, audit, comment classification and writer use Claude Sonnet 5.5.
The writer field is optional for older configs and defaults to ingest. #20 must
wire it into production registration, along with the other role settings.

Provider limits checked on 2026-10-30:

- [GPT-6.1 Sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol):
  1,050,000 context tokens and 128,000 maximum output tokens.
- [Claude models](https://platform.claude.com/docs/en/models/overview):
  Fable 5.1, Opus 5.5 and Sonnet 5.5 each have 1M context and 128K maximum output.
- [GPT effort](https://developers.openai.com/api/docs/guides/latest-model):
  GPT-6.1 Sol supports `max`.

All configured budgets also fit TL3's 150,000-token context setting. The proxy's
catalog lists IDs without limits, so these limits come from provider docs.
Run the explicit proxy smoke from the session, as Rob authorized for this task:
`python3 deploy/dl380g10/smoke-models.py ~/.config/tl3/tl3.toml`.
It loads the key in-process and prints only model IDs, HTTP status and token
counts. It sends one 16-token Chat Completions request per distinct configured
model, using attempt 2's effort for GPT-6.1 Sol. It never prints response bodies
or credentials. TL3 code tests still run only through PrismaBuild.
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
2. Give the tunnel token through the coordinator's secret prompt. The model key is installed.
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
narrow worker credentials to `model-key` only, and create the marker before starting `tl3d`. The daemon and loopback webhook
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

Reserve 192.168.1.107 in Rob's router DHCP settings if desired, so the status address stays stable.
