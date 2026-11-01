# TL3 config

`tl3-config` reads slice one's single TOML file. It is a Rust 2024 crate in the root workspace with one shared lockfile.
[example.toml](example.toml) cites the source of each setting.

`Config::parse(text)` rejects unknown fields at every level and validates the
values without accessing endpoints or secret files. `Config::load(path)` also
checks secret-file metadata. `validate_secret_files()` is available separately.
On Unix, each secret target must be a regular file with owner-read permission
and no group or other permissions. These checks never read the secret bytes.
They follow symlinks and do not replace runtime file-open checks.

`Config::load_status(path)` parses the same config and checks only the store
credential's metadata. It lets status run before model and App credentials are
available. Controller startup still uses full `Config::load`.
Optional `[operations]` holds `owner`, `repo`, and a positive `issue` number for
the Q17 operations issue. It does not add a served repository.

The example passes offline validation. Model names, maximum effort and output
budget, the app installation id, and the container digest are
deployment placeholders. Validation checks their shape, not service availability.
Hindsight's endpoint uses dl380g10's local port 8890. Port 8888 is celestia's tunnel.
The proxy owns the supported models and efforts. Budget means the worker's
per-response `max_completion_tokens`, including reasoning where supported.
It must be positive and no larger than the configured context.

The maker has exactly three attempts. Attempts 1 and 2 share a model and family,
attempt 2 has an explicit effort and at least attempt 1's budget, and attempt 3
uses a different family. The operator sets the proxy's maximum effort and budget
for attempt 2. An omitted or empty reviewer ladder defaults to maker attempts 1
and 3, in that order. `models.reviewer_for(maker_family)` returns the first
reviewer whose family differs from the current attempt's maker, ignoring ASCII
case, or `None` if none exists. Validation requires a different-family reviewer
for every family in the maker ladder, including with an explicit reviewer ladder.
Other role ladders must contain at least one model. No model names or effort
vocabulary are built into the crate.

The optional writer ladder defaults to ingest for older configs. Deployment
defaults, including the explicit writer choice, live in
`deploy/dl380g10/models.toml`. Production role registration belongs to #20.

`models.worker_model(step)` serializes exactly the worker's model fields:
`base_url`, `name`, `effort`, and `budget_tokens`. Family stays in config.
The supervisor supplies `models.key_file` and a worktree under `worktrees_root`
as separate worker request fields.

`github.app_credentials_file` points to the JSON written by
`deploy/github-app/`. At runtime the controller loads it as
`tl3_github::AppConfig`, whose fields are `id`, `slug`, `pem`, and
`webhook_secret`. This crate neither duplicates nor reads that secret type.
Each served repo supplies the separate installation id that `GitHub::new`
requires. Rob's numeric id authorizes label events, while his login is display
metadata. Optional `github.webhook_listen` requires a loopback IP and a nonzero
port. The simulation receiver defaults to `127.0.0.1:8081` when `--github-api`
enables it without a listen address. A local `cloudflared` process forwards the
tunnel there. Config does not resolve GitHub identities over the network.

`prismabuild.tools_dir` contains `pbrun.py`, `pbwait.py`, and the other published
tools. The harness image uses `sha256:<64 lowercase hex digits>` or
`repository@sha256:<64 lowercase hex digits>`, matching `harness.Container`.
PB's `content:sha256:` form is not a harness image reference.

`store` configures `tl3-store` with `host`, `port`, `database`, `user`,
`connect_timeout_secs` and an optional `password_file`. Omitting the section
uses local TCP port 5432 and database/user `tl3`. The store accepts only
`localhost`, `127.0.0.1`, `::1`, or an absolute Unix socket directory, without
TLS. A remote deployment needs a protected tunnel with a loopback endpoint.
Config validates an optional password file's metadata, and the store alone
reads its bytes at runtime. It never searches environment variables or default
credential files. Omitting the file uses the server's local authentication.

All paths are absolute. No tilde or environment-variable expansion occurs.
The worktrees root and tools directory need not exist during offline validation.
Allowances use positive integer seconds. Expiry triggers a look at the upstream,
not a work-duration deadline. Credit reserve and free rechecks allow zero;
W must be in 1..=3, matching the approved capacity and proof bounds. Rob approved
3 slots per repo once built and proven (Q29 amended, 2026-10-31). The coordinator
selects the deployment value; the example remains at 1. Slots and file locks are
granted together in ticket order. Disjoint leaves can run concurrently when agent capacity is
available, and overlapping leaves wait without holding a slot or any files.
Listen addresses are numeric socket addresses with a
fixed nonzero port. The example status address is `127.0.0.1:8033`; deployment
sets an explicit LAN address for the status page. HTTP endpoints allow LAN HTTP and reject embedded credentials,
queries, and fragments. Parse diagnostics omit source lines.

`limits.agent_pool` sets the kernel's simultaneous agent capacity, from 1 to 255.
It defaults to 1 for existing configuration files. `Controller::open` reconciles
both `limits.w` and `limits.agent_pool` with the stored world. Increases settle
through the kernel, then persist both capacities, admitted leaves and resulting
effects in one atomic commit. Reopening at the same capacities writes nothing.
A gate decrease refuses startup while any request in that repo is open. A pool
decrease refuses startup while any request is open. Idle decreases are recorded
in the event journal. Startup also retires ended looks when capacity changes.

Run `cargo test --locked --manifest-path crates/config/Cargo.toml` through
PrismaBuild from a clean commit. Tests use temporary fixture secret files only.

`workers` configures an absolute Python interpreter, a durable run root, and
Python arguments. Sync the worker environment once before starting the service:

```bash
uv sync --locked --project /srv/tl3/workers --python 3.14+gil
```

Set `workers.python` to `/srv/tl3/workers/.venv/bin/python`. The supervisor
launches it directly with `workers.command`, defaulting to `-m tl3_workers run`.
Per-run launches do not invoke uv or take its environment locks. Keep the run
root across controller restarts. Tests replace only the worker arguments and
model construction. Never configure a fixture launcher in deployment.
Optional `workers.tiktoken_cache_dir` is an absolute path. The dl380g10 service
wrapper exports it as `TIKTOKEN_CACHE_DIR` before launching the daemon.

`planning` defaults to the slice-one protected paths and named overhead estimates
of 5,000 prompt tokens, 20,000 making tokens and 5,000 review tokens.
`planning.overhead_tokens` is the calibration seam for later MLflow measurements.
It must contain at least one named nonnegative estimate.
`planning.protected_paths` can add patterns, but it must retain the defaults.

## Repository branches

Each `[[github.repos]]` entry accepts `branch_template` and `base_branch`.
Their defaults are `tl3/issue-{issue}-node-{node}` and `main`.
For PrismaBuild, use `branch_template = "prismabuild-{issue}-tl3-{node}"`.
For a repository based on `master`, set `base_branch = "master"`.

A template must contain `{issue}` and `{node}` exactly once, separated by a
literal containing a nondigit. Other placeholders and invalid Git refs are
rejected. Issue numbers are positive; node numbers can be zero. Both use
canonical decimal digits. Formatting and recognition share the same parser.
Publication writes `Refs #N` for the request's issue. TL3 closes the issue after
all leaves finish.

## Repository check images

Each `[[github.repos]]` entry accepts an optional `container_image`. Omission
uses `[prismabuild] container_image`. Both settings require an immutable image
ID or manifest digest in the formats above. Repository overrides match the
owner and name without regard to ASCII case, and the installation ID exactly.
The controller stages the selected image into the harness request and seals
that same image as the PrismaBuild placement requirement.

For example, keep the sandbox on the global image and set the PB override:

```toml
[[github.repos]]
owner = "RobTand"
name = "prismabuild"
installation_id = 123 # Replace with the installation's numeric ID.
container_image = "sha256:<64 lowercase hex digits>"
```

Build and qualify an image with the steps in [the harness guide](../../harness/README.md#prepare-and-roll-a-repository-image).
The image must already exist on every eligible worker. PrismaBuild checks
availability before claiming an action and never transfers or pulls images.
